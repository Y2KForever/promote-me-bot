use crate::models::{self, update_tiktok_last_post_id, update_tiktok_tokens, TikTokRegistration};
use crate::oauth::refresh_tiktok_token;
use crate::tasks::TaskContext;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use twilight_http::request::Request;
use twilight_http::routing::Route;
use twilight_model::channel::Message;

#[derive(Deserialize, Debug)]
struct TiktokVideoApiResponse<T> {
    data: Option<T>,
    error: TiktokErrorDetails,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct TiktokDataPayload {
    videos: Vec<TiktokVideoResponse>,
    cursor: i64,
    has_more: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct TiktokErrorDetails {
    code: String,
    message: String,
    log_id: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct TiktokVideoResponse {
    id: String,
    share_url: String,
    video_description: String,
    title: String,
    embed_link: String,
}

pub async fn run_tiktok_poll_loop(ctx: Arc<TaskContext>) -> anyhow::Result<()> {
    let mut interval = tokio::time::interval(Duration::from_secs(60 * 5));

    loop {
        interval.tick().await;
        println!("[TikTok Task] Starting poll cycle...");

        println!("[TikTok Task] Fetching registrations..");
        let regs = match models::get_all_tiktok_registrations(&ctx.app_state.db).await {
            Ok(regs) => {
                println!(
                    "[Tiktok Task] Successfully fetched {} registrations.",
                    regs.len()
                );
                regs
            }
            Err(e) => {
                println!(
                    "[Tiktok Task] CRITICAL ERROR fetching Tiktok registrations: {:?}",
                    e
                );
                continue;
            }
        };

        println!("[RSS Task] Checking {} feeds...", regs.len());

        for mut reg in regs {
            if let Err(e) = check_profile(&ctx, &mut reg).await {
                println!(
                    "[Tiktok Task] Error checking user {}: {:?}",
                    reg.tiktok_display_name, e
                );
            }
        }
        println!("[Tiktok Task] Finished poll cycle.");
    }
}

async fn check_profile(ctx: &Arc<TaskContext>, reg: &mut TikTokRegistration) -> anyhow::Result<()> {
    let api_response = fetch_video_list(ctx, &reg.access_token).await?;

    match api_response.error.code.as_str() {
        "ok" => {
            if let Some(data) = api_response.data {
                return process_video_response(ctx, reg, &data).await;
            }
            Ok(())
        }

        "access_token_invalid" | "2190008" | "105002" | "40105" => {
            println!(
                "[Tiktok Task] Token expired for {} ({}). Refreshing...",
                reg.tiktok_display_name, api_response.error.code
            );

            let new_token_data =
                refresh_tiktok_token(&ctx.app_state.http, reg.refresh_token.clone()).await?;

            reg.access_token = new_token_data.access_token.clone();
            reg.refresh_token = new_token_data.refresh_token.clone();

            update_tiktok_tokens(
                &ctx.app_state.db,
                &reg.pk,
                &reg.sk,
                &new_token_data.access_token,
                &new_token_data.refresh_token,
            )
            .await?;

            println!(
                "[Tiktok Task] Retrying API call for {}.",
                reg.tiktok_display_name
            );

            let retry_response = fetch_video_list(ctx, &reg.access_token).await?;

            if retry_response.error.code == "ok" {
                if let Some(data) = retry_response.data {
                    return process_video_response(ctx, reg, &data).await;
                }
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "[Tiktok API] API call failed after refresh for {} (Code: {}, Message: {})",
                    reg.tiktok_display_name,
                    retry_response.error.code,
                    retry_response.error.message
                ))
            }
        }
        _ => Err(anyhow::anyhow!(
            "[Tiktok API] Unhandled API Error for {} (Code: {}, Message: {})",
            reg.tiktok_display_name,
            api_response.error.code,
            api_response.error.message
        )),
    }
}

async fn fetch_video_list(
    ctx: &Arc<TaskContext>,
    access_token: &str,
) -> anyhow::Result<TiktokVideoApiResponse<TiktokDataPayload>> {
    let url = "https://open.tiktokapis.com/v2/video/list/";
    let payload = json!({
      "max_count": 1,
    });

    let response = ctx
        .app_state
        .http
        .post(url)
        .bearer_auth(access_token)
        .json(&payload)
        .query(&[("fields", "id,share_url,video_description,title,embed_link")])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await?;
        println!("Error Body: {}", error_body);
        return Err(anyhow::anyhow!("[TikTok Task] API HTTP Error: {}", status));
    }

    let api_response = response
        .json::<TiktokVideoApiResponse<TiktokDataPayload>>()
        .await?;

    Ok(api_response)
}

async fn process_video_response(
    ctx: &Arc<TaskContext>,
    reg: &mut TikTokRegistration,
    data: &TiktokDataPayload,
) -> anyhow::Result<()> {
    let latest_video = match data.videos.get(0) {
        Some(video) => video,
        None => {
            println!(
                "[Tiktok Task] No videos found for {}",
                reg.tiktok_display_name
            );
            return Ok(());
        }
    };

    let latest_id = &latest_video.id;
    let has_new_post = reg.last_post_guid.as_deref() != Some(latest_id);

    if has_new_post {
        println!(
            "[Tiktok Task] New video found for {}",
            reg.tiktok_display_name
        );

        let username = &reg.tiktok_username;
        let channel_id = reg.channel_id;
        let route = Route::CreateMessage { channel_id };

        let message = format!(
            "{}\n\n",
            format!("https://www.tiktok.com/@{}/video/{}", &username, &latest_id)
        );

        let body = json!({ "content": message });
        let body_bytes = serde_json::to_vec(&body)?;

        let request = Request::builder(&route).body(body_bytes).build()?;

        if let Err(e) = ctx.client.request::<Message>(request).await {
            println!(
                "[Tiktok] Failed to send the Discord message for {}: {}",
                reg.tiktok_display_name, e
            );
            return Err(e.into());
        }

        let pk = reg.pk.clone();
        let sk = reg.sk.clone();

        update_tiktok_last_post_id(&ctx.app_state.db, &pk, &sk, latest_id).await?;

        reg.last_post_guid = Some(latest_id.clone());
    }

    Ok(())
}

pub async fn revoke_tiktok_access(ctx: &Arc<AppState>, access_token: &str) -> anyhow::Result<()> {
    let url = "https://open.tiktokapis.com/v2/oauth/revoke/";
    let client_key = env!("TIKTOK_CLIENT_KEY");
    let client_secret = env!("TIKTOK_CLIENT_SECRET");

    let payload = [
        ("client_key", client_key),
        ("client_secret", client_secret),
        ("token", access_token),
    ];

    let response = ctx
        .http
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .form(&payload)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("[TikTok Task] Failed to send request: {}", e))?;

    if response.status().is_success() {
        return Ok(());
    }

    let status = response.status();

    let error_body_text = response.text().await.map_err(|e| {
        anyhow::anyhow!(
            "[TikTok Task] API Error: {} - Failed to read error body: {}",
            status,
            e
        )
    })?;

    if let Ok(parsed_error) = serde_json::from_str::<TiktokErrorDetails>(&error_body_text) {
        Err(anyhow::anyhow!(
            "[TikTok Task] API Error: {} ({})",
            parsed_error.code,
            parsed_error.message
        ))
    } else {
        Err(anyhow::anyhow!(
            "[TikTok Task] API HTTP Error: {} - Body: {}",
            status,
            error_body_text
        ))
    }
}
