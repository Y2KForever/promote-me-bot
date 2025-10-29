use crate::models;
use crate::tasks::TaskContext;
use crate::TwitchEventInfo;

use dotenvy_macro::dotenv;
use once_cell::sync::Lazy;
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use serenity::all::{Colour, CreateActionRow, CreateButton, CreateEmbedAuthor};
use serenity::builder::{CreateEmbed, CreateMessage};
use serenity::model::id::ChannelId;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;

struct CachedAppToken {
    token: String,
    expires_at: Instant,
}

static TWITCH_APP_TOKEN: Lazy<Mutex<Option<CachedAppToken>>> = Lazy::new(|| Mutex::new(None));

#[derive(Deserialize)]
struct AppTokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize, Debug)]
struct UserApiResponse {
    data: Vec<UserData>,
}

#[derive(Deserialize, Debug)]
struct UserData {
    profile_image_url: String,
}

#[derive(Deserialize, Debug)]
struct GamesApiResponse {
    data: Vec<GameData>,
}

#[derive(Deserialize, Debug)]
struct GameData {
    box_art_url: String,
}

async fn get_app_access_token(http: &ReqwestClient) -> anyhow::Result<String> {
    let mut cache_guard = TWITCH_APP_TOKEN.lock().await;

    if let Some(cached) = cache_guard.as_ref() {
        if cached.expires_at > (Instant::now() + Duration::from_secs(60)) {
            println!("[Twitch Auth] Using chached App access token");
            return Ok(cached.token.clone());
        } else {
            println!("[Twitch Auth] App access token expired");
        }
    }
    println!("[Twitch Auth] Requesting new app access token...");
    let client_id = dotenv!("TWITCH_CLIENT_ID");
    let client_secret = dotenv!("TWITCH_CLIENT_SECRET");
    let token_url = "https://id.twitch.tv/oauth2/token";

    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "client_credentials"),
    ];

    let resp = http.post(token_url).form(&params).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await?;
        *cache_guard = None;
        return Err(anyhow::anyhow!(
            "Failed to get twitch app access token: Status {}, Body: {}",
            status,
            body
        ));
    }

    let token_resp: AppTokenResponse = resp.json().await?;
    let expires_at = Instant::now() + Duration::from_secs(token_resp.expires_in);

    println!("[Twitch Auth] Got new app access token");
    let new_token = token_resp.access_token;

    *cache_guard = Some(CachedAppToken {
        token: new_token.clone(),
        expires_at: expires_at,
    });

    Ok(new_token)
}

pub async fn run_twitch_event_handler(
    ctx: Arc<TaskContext>,
    mut receiver: Receiver<TwitchEventInfo>,
) {
    println!("Twitch Event Handler task started.");
    while let Some(event) = receiver.recv().await {
        println!(
            "Processing stream.online event for {}",
            event.broadcaster_user_login
        );

        let reg_result =
            models::get_twitch_reg_by_login(&ctx.app_state.db, &event.broadcaster_user_login).await;

        let reg = match reg_result {
            Ok(Some(r)) => r,
            Ok(None) => {
                println!(
                    "Received event for non-registered Twitch user login {}",
                    event.broadcaster_user_login
                );
                continue;
            }
            Err(e) => {
                println!(
                    "DB error fetching Twitch user by login {}: {}",
                    event.broadcaster_user_login, e
                );
                continue;
            }
        };

        println!("[Twitch Webhook] Incoming event: {:?}", event);

        let title = event.title.unwrap_or_else(|| "".to_string());
        let category = event.category_name.clone();
        let game_name = event
            .category_name
            .unwrap_or_else(|| "No category".to_string());
        let stream_url = format!("https://twitch.tv/{}", event.broadcaster_user_login);

        let profile_pic_url =
            get_twitch_profile_picture(&ctx.app_state.http, &event.broadcaster_user_id).await;
        let game_box_art_url =
            get_twitch_game_box_art(&ctx.app_state.http, category.as_deref()).await;
        let stream_thumbnail_url = format!(
            "https://static-cdn.jtvnw.net/previews-ttv/live_user_{}-1920x1080.jpg",
            event.broadcaster_user_login
        );

        let mut embed_builder = CreateEmbed::new()
            .author(
                CreateEmbedAuthor::new(&event.broadcaster_user_name)
                    .url(&stream_url)
                    .icon_url(profile_pic_url.unwrap_or_default()),
            )
            .title(&title)
            .url(&stream_url)
            .color(Colour::PURPLE)
            .field("Playing", game_name, true);

        if let Some(thumbnail) = game_box_art_url {
            embed_builder = embed_builder.thumbnail(thumbnail);
        }

        embed_builder = embed_builder.image(stream_thumbnail_url);

        let button = CreateButton::new_link(&stream_url).label("Watch stream");
        let action_row = CreateActionRow::Buttons(vec![button]);

        let message_builder = CreateMessage::new()
            .embed(embed_builder)
            .components(vec![action_row]);

        let channel = ChannelId::new(reg.discord_channel_id);
        if let Err(e) = channel
            .send_message(ctx.http.as_ref(), message_builder)
            .await
        {
            println!(
                "Failed to send Discord message for {}: {}",
                event.broadcaster_user_login, e
            );
        }
    }

    println!("Twitch Event Handler task finished.");
}

async fn get_twitch_profile_picture(http: &ReqwestClient, user_id: &str) -> Option<String> {
    let Ok(token) = get_app_access_token(http).await else {
        println!("[Twitch API] Failed to get app token for porifle picture fetch.");
        return None;
    };

    let client_id = dotenv!("TWITCH_CLIENT_ID");
    let url = format!("https://api.twitch.tv/helix/users?id={}", user_id);

    match http
        .get(&url)
        .header("Client-Id", client_id)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                println!(
                    "[Twitch API] /helix/users request failed: Status {}",
                    resp.status()
                );
                return None;
            }
            match resp.json::<UserApiResponse>().await {
                Ok(api_response) => api_response
                    .data
                    .first()
                    .map(|user| user.profile_image_url.clone()),
                Err(e) => {
                    println!("[Twitch API] failed to parse /helix/users response: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            println!("[Twitch API] Network error /helix/users: {}", e);
            None
        }
    }
}

async fn get_twitch_game_box_art(
    http: &ReqwestClient,
    category_name: Option<&str>,
) -> Option<String> {
    let Some(id) = category_name else { return None };
    if id.is_empty() {
        return None;
    };

    let Ok(token) = get_app_access_token(http).await else {
        println!("[Twitch API] Failed to get app token for porifle picture fetch.");
        return None;
    };

    let client_id = dotenv!("TWITCH_CLIENT_ID");
    let url = format!("https://api.twitch.tv/helix/games?name={}", id);

    match http
        .get(&url)
        .header("Client-Id", client_id)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                println!(
                    "[Twitch API] /helix/games/request failed: Status {}",
                    resp.status()
                );
                return None;
            }
            match resp.json::<GamesApiResponse>().await {
                Ok(api_response) => api_response.data.first().map(|game| {
                    game.box_art_url
                        .replace("{width}", "285")
                        .replace("{height}", "380")
                }),
                Err(e) => {
                    println!("[Twitch API] failed to parse /helix/games response: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            println!("[Twitch API] network error calling /helix/games: {}", e);
            None
        }
    }
}
