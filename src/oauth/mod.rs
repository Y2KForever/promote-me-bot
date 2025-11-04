use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use dotenvy_macro::dotenv;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{collections::HashMap, net::SocketAddr};

use crate::{
    models::{self, TikTokRegistration, TwitchRegistration},
    AppState, TwitchEventInfo,
};
use reqwest::Client as ReqwestClient;
use serenity::http::Http;
use serenity::model::id::ChannelId;

use serenity::builder::CreateMessage;

use hmac::{Hmac, Mac};
use sha2::Sha256;

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

#[derive(Deserialize, Debug)]
struct TwitchTokenResponse {
    access_token: String,
    refresh_token: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct TwitchUser {
    id: String,
    login: String,
    display_name: String,
}

#[derive(Serialize, Debug)]
struct Broadcaster {
    id: String,
    login: String,
}

#[derive(Deserialize, Debug)]
struct TwitchUsersResponse {
    data: Vec<TwitchUser>,
}

#[derive(Deserialize, Debug, Clone)]
struct TwitchSubscription {
    #[serde(rename = "type")]
    sub_type: String,
}

#[derive(Deserialize, Debug, Clone)]
struct TwitchWebhookPayload {
    subscription: TwitchSubscription,
    event: Option<TwitchEventInfo>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
pub struct TikTokTokenResponse {
    pub access_token: String,
    pub expires_in: i64,
    pub open_id: String,
    pub refresh_token: String,
    pub refresh_expires_in: i64,
    pub scope: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct TikTokUser {
    pub open_id: String,
    pub union_id: String,
    pub display_name: String,
    pub username: String,
    pub avatar_url: String,
}

#[derive(Deserialize, Debug)]
pub struct TikTokUserData {
    pub user: TikTokUser,
}

#[derive(Deserialize, Debug)]
pub struct TikTokError {
    pub code: String,
    pub message: String,
}

#[derive(Deserialize, Debug)]
pub struct TikTokUserResponse {
    pub data: TikTokUserData,
    pub error: TikTokError,
}

#[derive(Clone)]
pub struct WebServerState {
    pub app_state: Arc<AppState>,
    pub discord_http: Arc<Http>,
}

pub async fn start_server(state: WebServerState, port: u16) {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = Router::new()
        .route("/auth/twitch/callback", get(twitch_callback))
        .route("/auth/tiktok/callback", get(tiktok_callback))
        .route("/webhook/twitch", post(twitch_webhook_receiver))
        .with_state(state);

    println!("OAuth & Webhook server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn twitch_callback(
    Query(query): Query<CallbackQuery>,
    State(state): State<WebServerState>,
) -> impl IntoResponse {
    let parts: Vec<&str> = query.state.split('_').collect();
    if parts.len() != 2 {
        return (StatusCode::BAD_REQUEST, "Invalid state").into_response();
    }

    let guild_id: u64 = match parts[1].parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid Guild ID in state").into_response(),
    };

    let target_channel_id = match models::get_server_config(&state.app_state.db, guild_id).await {
        Ok(Some(config)) => config.notification_channel_id,
        Ok(None) => {
            return (StatusCode::OK, "Registration failed: Server notification channel not configured. Ask an admin to run /config channel.").into_response();
        }
        Err(e) => {
            println!("DB lookup error for Guild {}: {}", guild_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error during channel lookup",
            )
                .into_response();
        }
    };
    let channel = ChannelId::new(target_channel_id);

    let token_response: TwitchTokenResponse =
        match exchange_twitch_code(&state.app_state.http, query.code).await {
            Ok(resp) => resp,
            Err(e) => {
                println!("Twitch code exchange error: {}", e);
                let _ = channel
                    .send_message(
                        state.discord_http.as_ref(),
                        CreateMessage::new().content("Error: Failed to exchange Twitch token."),
                    )
                    .await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to exchange token",
                )
                    .into_response();
            }
        };

    let user_data: TwitchUser =
        match get_twitch_user(&state.app_state.http, &token_response.access_token).await {
            Ok(user) => user,
            Err(e) => {
                println!("Twitch get user error: {}", e);
                let _ = channel
                    .send_message(
                        state.discord_http.as_ref(),
                        CreateMessage::new().content("Error: Failed to get Twitch user data."),
                    )
                    .await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to get user data")
                    .into_response();
            }
        };

    let reg = TwitchRegistration {
        pk: format!("GUILD#{}", guild_id),
        sk: format!("TWITCH#{}", user_data.id),
        gsi1pk: Some("TWITCH_LOOKUP".to_string()),
        gsi1sk: Some(format!("LOGIN#{}", user_data.login.to_lowercase())),
        item_type: "TWITCH_REG".to_string(),
        twitch_user_id: user_data.id.clone(),
        twitch_login: user_data.login.clone(),
        discord_channel_id: target_channel_id,
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
    };

    match models::save_twitch_registration(&state.app_state.db, reg).await {
        Ok(_) => {
            let webhook_register_url = dotenv!("WEBHOOK_REGISTER_URL");
            let broadcaster_data = Broadcaster {
                id: user_data.id.clone(),
                login: user_data.login.clone(),
            };
            let streamers = vec![broadcaster_data];
            let client = &state.app_state.http;

            match client
                .post(webhook_register_url)
                .json(&streamers)
                .send()
                .await
            {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let error_text = resp
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        println!(
                            "WARNING: Failed to register webhook for {}. Status: {}, Error: {}",
                            user_data.login, status, error_text
                        );
                    } else {
                        println!(
                            "Successfully triggered webhook registration for {}",
                            user_data.login
                        );
                    }
                }
                Err(e) => {
                    println!(
                        "WARNING: Error calling webhook registration endpoint for {}: {}",
                        user_data.login, e
                    );
                }
            }

            let success_msg = format!(
                "✅ Successfully registered Twitch channel: `{}`. Updates will post to <#{}>.",
                user_data.login, target_channel_id
            );
            let builder = CreateMessage::new().content(success_msg);
            if let Err(e) = channel
                .send_message(state.discord_http.as_ref(), builder)
                .await
            {
                println!("Failed to send Discord success message: {}", e);
            }
            (StatusCode::OK, "Success! You can close this window.").into_response()
        }
        Err(e) => {
            println!("DB save error: {}", e);
            let _ = channel
                .send_message(
                    state.discord_http.as_ref(),
                    CreateMessage::new().content("Error: Failed to save registration to database."),
                )
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error: Failed to save registration.",
            )
                .into_response()
        }
    }
}

async fn exchange_twitch_code(
    http: &ReqwestClient,
    code: String,
) -> anyhow::Result<TwitchTokenResponse> {
    let client_id = dotenv!("TWITCH_CLIENT_ID");
    let client_secret = dotenv!("TWITCH_CLIENT_SECRET");
    let base_url = dotenv!("BASE_URL");
    let redirect_uri = format!("{}/auth/twitch/callback", base_url);

    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code.as_str()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri.as_str()),
    ];

    let resp = http
        .post("https://id.twitch.tv/oauth2/token")
        .form(&params)
        .send()
        .await?;

    let status = resp.status();
    let error_body = resp.text().await?;

    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Twitch token error: Status {}, Body: {}",
            status,
            error_body
        ));
    }
    Ok(serde_json::from_str(&error_body)?)
}

async fn get_twitch_user(http: &ReqwestClient, token: &str) -> anyhow::Result<TwitchUser> {
    let client_id = dotenv!("TWITCH_CLIENT_ID");

    let resp_result = http
        .get("https://api.twitch.tv/helix/users")
        .bearer_auth(token)
        .header("Client-Id", client_id)
        .send()
        .await;

    let resp = match resp_result {
        Ok(r) => r,
        Err(e) => {
            println!("[DEBUG] Network error fetching Twitch user: {}", e);
            return Err(e.into());
        }
    };

    let status = resp.status();
    let raw_body = resp.text().await?;

    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Twitch user request failed: Status {}, Body: {}",
            status,
            raw_body
        ));
    }

    let user_resp: TwitchUsersResponse = serde_json::from_str(&raw_body)?;

    user_resp
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No user data found in Twitch response"))
}

async fn twitch_webhook_receiver(
    State(state): State<WebServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = dotenv!("TWITCH_WEBHOOK_SECRET");

    if !verify_twitch_signature(&headers, &body, &secret) {
        println!("Webhook verification FAILED!");
        return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
    }

    if headers
        .get("Twitch-Eventsub-Message-Type")
        .map_or(false, |v| v == "webhook_callback_verification")
    {
        #[derive(Deserialize)]
        struct Challenge {
            challenge: String,
        }
        if let Ok(json) = serde_json::from_slice::<Challenge>(&body) {
            println!("Webhook verification challenge successful.");
            return (StatusCode::OK, json.challenge).into_response();
        }
        return (StatusCode::BAD_REQUEST, "Invalid challenge").into_response();
    }

    let payload: TwitchWebhookPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            println!("Failed to parse webhook body: {}", e);
            return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response();
        }
    };

    if payload.subscription.sub_type == "stream.online" {
        if let Some(event_info) = payload.event {
            if let Err(e) = state
                .app_state
                .twitch_event_sender
                .send(event_info.clone())
                .await
            {
                println!("Failed to send Twitch event to handler task: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to queue event")
                    .into_response();
            } else {
                println!(
                    "Queued stream.online event for {}",
                    event_info.broadcaster_user_login
                );
            }
        } else {
            println!("Received stream.online webhook with missing event data.");
            return (StatusCode::BAD_REQUEST, "Missing event data").into_response();
        }
    }

    (StatusCode::OK, "Event received").into_response()
}

fn verify_twitch_signature(headers: &HeaderMap, body: &Bytes, secret: &str) -> bool {
    let Some(message_id) = headers.get("Twitch-Eventsub-Message-Id") else {
        return false;
    };
    let Some(timestamp) = headers.get("Twitch-Eventsub-Message-Timestamp") else {
        return false;
    };
    let Some(signature_header) = headers.get("Twitch-Eventsub-Message-Signature") else {
        return false;
    };

    let signature_str = match signature_header.to_str() {
        Ok(s) => s.strip_prefix("sha256=").unwrap_or(s),
        Err(_) => return false,
    };

    let Ok(expected_hash) = hex::decode(signature_str) else {
        println!("Webhook signature was not valid hex");
        return false;
    };

    let mut message = Vec::new();
    message.extend_from_slice(message_id.as_bytes());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key error");

    mac.update(&message);

    mac.verify_slice(&expected_hash).is_ok()
}

async fn exchange_tiktok_code(
    http: &reqwest::Client,
    code: String,
) -> anyhow::Result<TikTokTokenResponse> {
    let client_key = dotenv!("TIKTOK_CLIENT_KEY");
    let client_secret = dotenv!("TIKTOK_CLIENT_SECRET");
    let base_url = dotenv!("BASE_URL");
    let redirect_uri = format!("{}/auth/tiktok/callback", base_url);

    let token_url = "https://open.tiktokapis.com/v2/oauth/token/";

    let mut params = HashMap::new();
    params.insert("client_key", client_key);
    params.insert("client_secret", client_secret);
    params.insert("code", &code);
    params.insert("grant_type", "authorization_code");
    params.insert("redirect_uri", &redirect_uri);

    let resp = http.post(token_url).form(&params).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_text = resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow::anyhow!("TikTok Token API Error ({}): {}", status, error_text).into());
    }

    let token_response = resp.json::<TikTokTokenResponse>().await?;
    Ok(token_response)
}

async fn get_tiktok_user(http: &reqwest::Client, access_token: &str) -> anyhow::Result<TikTokUser> {
    let user_info_url = "https://open.tiktokapis.com/v2/user/info/";

    let fields = "open_id,union_id,display_name,avatar_url,username";

    let resp = http
        .get(user_info_url)
        .bearer_auth(access_token)
        .query(&[("fields", fields)])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_text = resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(
            anyhow::anyhow!("TikTok User Info API Error ({}): {}", status, error_text).into(),
        );
    }

    let user_response = resp.json::<TikTokUserResponse>().await?;

    if user_response.error.code != "ok" {
        return Err(anyhow::anyhow!(
            "TikTok User Info API Logic Error: {}",
            user_response.error.message
        )
        .into());
    }

    Ok(user_response.data.user)
}

async fn tiktok_callback(
    Query(query): Query<CallbackQuery>,
    State(state): State<WebServerState>,
) -> impl IntoResponse {
    let parts: Vec<&str> = query.state.split('_').collect();
    if parts.len() != 2 {
        return (StatusCode::BAD_REQUEST, "Invalid state").into_response();
    }

    let guild_id: u64 = match parts[1].parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid Guild ID in state").into_response(),
    };

    let target_channel_id = match models::get_server_config(&state.app_state.db, guild_id).await {
        Ok(Some(config)) => config.notification_channel_id,
        Ok(None) => {
            return (StatusCode::OK, "Registration failed: Server notification channel not configured. Ask an admin to run /config channel.").into_response();
        }
        Err(e) => {
            println!("DB lookup error for Guild {}: {}", guild_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database error during channel lookup",
            )
                .into_response();
        }
    };
    let channel = ChannelId::new(target_channel_id);

    let token_response: TikTokTokenResponse =
        match exchange_tiktok_code(&state.app_state.http, query.code).await {
            Ok(resp) => resp,
            Err(e) => {
                println!("TikTok code exchange error: {}", e);
                let _ = channel
                    .send_message(
                        state.discord_http.as_ref(),
                        CreateMessage::new().content("Error: Failed to exchange TikTok token."),
                    )
                    .await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to exchange token",
                )
                    .into_response();
            }
        };

    let user_data: TikTokUser =
        match get_tiktok_user(&state.app_state.http, &token_response.access_token).await {
            Ok(user) => user,
            Err(e) => {
                println!("TikTok get user error: {}", e);
                let _ = channel
                    .send_message(
                        state.discord_http.as_ref(),
                        CreateMessage::new().content("Error: Failed to get TikTok user data."),
                    )
                    .await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to get user data")
                    .into_response();
            }
        };

    let reg = TikTokRegistration {
        pk: format!("GUILD#{}", guild_id),
        sk: format!("TIKTOK#{}", user_data.open_id),
        gsi1pk: Some("TIKTOK_LOOKUP".to_string()),
        gsi1sk: Some(format!("NAME#{}", user_data.username.to_lowercase())),
        item_type: "TIKTOK_REG".to_string(),
        tiktok_open_id: user_data.open_id.clone(),
        tiktok_display_name: user_data.display_name.clone(),
        tiktok_username: user_data.username.clone(),
        channel_id: target_channel_id,
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        last_post_guid: None,
    };

    match models::save_tiktok_registration(&state.app_state.db, reg).await {
        Ok(_) => {
            println!(
                "Successfully registered TikTok user: {}",
                user_data.display_name
            );

            let success_msg = format!(
                "✅ Successfully registered TikTok account: `{}`. Updates will post to <#{}>.",
                user_data.display_name, target_channel_id
            );
            let builder = CreateMessage::new().content(success_msg);
            if let Err(e) = channel
                .send_message(state.discord_http.as_ref(), builder)
                .await
            {
                println!("Failed to send Discord success message: {}", e);
            }
            (StatusCode::OK, "Success! You can close this window.").into_response()
        }
        Err(e) => {
            println!("DB save error: {}", e);
            let _ = channel
                .send_message(
                    state.discord_http.as_ref(),
                    CreateMessage::new().content("Error: Failed to save registration to database."),
                )
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error: Failed to save registration.",
            )
                .into_response()
        }
    }
}

pub async fn refresh_tiktok_token(
    http: &reqwest::Client,
    refresh_token: String,
) -> anyhow::Result<TikTokTokenResponse> {
    // Assuming TikTokTokenResponse is your struct
    let client_key = dotenv!("TIKTOK_CLIENT_KEY");
    let client_secret = dotenv!("TIKTOK_CLIENT_SECRET");
    let token_url = "https://open.tiktokapis.com/v2/oauth/token/";

    let mut params = HashMap::new();
    params.insert("client_key", client_key.to_string());
    params.insert("client_secret", client_secret.to_string());
    params.insert("grant_type", "refresh_token".to_string());
    params.insert("refresh_token", refresh_token);

    let resp = http.post(token_url).form(&params).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_text = resp
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow::anyhow!(
            "TikTok Token Refresh API Error ({}): {}",
            status,
            error_text
        ));
    }

    let token_response = resp.json::<TikTokTokenResponse>().await?;
    Ok(token_response)
}
