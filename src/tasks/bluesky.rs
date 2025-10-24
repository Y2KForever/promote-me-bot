use crate::models;
use crate::tasks::TaskContext;
use futures_util::StreamExt;
use serde::Deserialize;
use serenity::builder::CreateMessage;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLockReadGuard;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use dotenvy_macro::dotenv;
use once_cell::sync::Lazy;
use reqwest::{header, Client as ReqwestClient, StatusCode};
use tokio::sync::Mutex;

const JETSTREAM_BASE_URL: &str =
    "wss://jetstream2.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post";

static BOT_AUTH_TOKEN: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

#[derive(serde::Serialize)]
struct CreateSessionRequest<'a> {
    identifier: &'a str,
    password: &'a str,
}
#[derive(Deserialize)]
struct CreateSessionResponse {
    #[serde(rename = "accessJwt")]
    access_jwt: String,
}
#[derive(Deserialize, Debug)]
struct BlueskyProfile {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Deserialize, Debug)]
struct JetstreamMessage {
    did: String,
    kind: String,
    commit: Option<JetstreamCommitData>,
}
#[derive(Deserialize, Debug)]
struct JetstreamCommitData {
    operation: String,
    collection: String,
    rkey: String,
    record: Option<JetstreamPostData>,
}
#[derive(Deserialize, Debug)]
struct JetstreamPostData {
    #[serde(rename = "$type")]
    record_type: String,
    text: String,
}

async fn get_bot_auth_token(http: &ReqwestClient) -> anyhow::Result<String> {
    let mut token_guard = BOT_AUTH_TOKEN.lock().await;
    if let Some(token) = token_guard.as_deref() {
        return Ok(token.to_string());
    }

    println!("[Bluesky Auth] Bot token not cached, logging in...");
    let handle = dotenv!("BSKY_BOT_HANDLE");
    let password = dotenv!("BSKY_BOT_PASSWORD");
    let req_body = CreateSessionRequest {
        identifier: handle,
        password,
    };
    let resp = http
        .post("https://bsky.social/xrpc/com.atproto.server.createSession")
        .json(&req_body)
        .send()
        .await?;

    let status = resp.status();
    let raw_body = resp.text().await?;

    if !status.is_success() {
        println!("[Bluesky Auth] Bot login failed: Status {}", status);
        *token_guard = None;
        return Err(anyhow::anyhow!(
            "Bluesky bot login failed: Status {}, Body: {}",
            status,
            raw_body
        ));
    }

    let session: CreateSessionResponse = match serde_json::from_str(&raw_body) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "[Bluesky Auth] CRITICAL: Failed to parse login response JSON: {}",
                e
            );
            *token_guard = None;
            return Err(e.into());
        }
    };
    println!("[Bluesky Auth] Bot login successful.");
    let token = session.access_jwt;
    *token_guard = Some(token.clone());
    Ok(token)
}

async fn get_bsky_profile_display_name(
    http_client: &ReqwestClient,
    did: &str,
    fallback_handle: &str,
) -> String {
    let url = format!(
        "https://bsky.social/xrpc/app.bsky.actor.getProfile?actor={}",
        did
    );
    let token = match get_bot_auth_token(http_client).await {
        Ok(t) => t,
        Err(e) => {
            println!(
                "[Bluesky Profile] Failed to get bot auth token: {}. Falling back.",
                e
            );
            return fallback_handle.to_string();
        }
    };

    match http_client
        .get(&url)
        .header(header::AUTHORIZATION, format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                match response.json::<BlueskyProfile>().await {
                    Ok(profile) => profile
                        .display_name
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| fallback_handle.to_string()),
                    Err(e) => {
                        println!("[Bluesky Profile] Parse error: {}", e);
                        fallback_handle.to_string()
                    }
                }
            } else if status == StatusCode::UNAUTHORIZED {
                println!("[Bluesky Profile] Bot token invalid (401). Clearing cache.");
                *BOT_AUTH_TOKEN.lock().await = None;
                fallback_handle.to_string()
            } else {
                println!("[Bluesky Profile] Fetch error: Status {}", status);
                fallback_handle.to_string()
            }
        }
        Err(e) => {
            println!("[Bluesky Profile] Network error: {}", e);
            fallback_handle.to_string()
        }
    }
}

pub async fn run_bluesky_firehose_loop(ctx: Arc<TaskContext>) {
    loop {
        println!(
            "[Bluesky Task] Connecting to Jetstream (unfiltered DIDs): {}",
            JETSTREAM_BASE_URL
        );

        match connect_and_listen_jetstream(&ctx, JETSTREAM_BASE_URL).await {
            Ok(_) => println!("[Bluesky Task] Listener exited cleanly (unexpected)."),
            Err(e) => println!("[Bluesky Task] Connection error: {}. Reconnecting...", e),
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn connect_and_listen_jetstream(ctx: &Arc<TaskContext>, url: &str) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(url).await?;
    println!("[Bluesky Task] Successfully connected.");
    let (_write, mut read) = ws_stream.split();

    while let Some(message_result) = read.next().await {
        match message_result {
            Ok(ws_message) => {
                match ws_message {
                    WsMessage::Text(text_data) => {
                        if let Err(e) = handle_jetstream_message(ctx, &text_data).await {
                            println!("[Bluesky Task] Error handling message content: {}", e);
                        }
                    }
                    WsMessage::Close(e) => {
                        println!("[Bluesky Task] Connection closed: {:?}", e);
                        return Err(anyhow::anyhow!("Connection closed by server"));
                    }
                    _ => {} // Ignore Ping, Pong, Binary, Frame
                }
            }
            Err(e) => {
                println!("[Bluesky Task] WebSocket read error: {}", e);
                return Err(e.into());
            }
        }
    }
    Err(anyhow::anyhow!("WebSocket stream ended unexpectedly"))
}

async fn handle_jetstream_message(ctx: &Arc<TaskContext>, data: &str) -> anyhow::Result<()> {
    let message: JetstreamMessage = match serde_json::from_str(data) {
        Ok(msg) => msg,
        Err(_e) => {
            return Ok(());
        }
    };

    if message.kind == "commit" {
        let did = message.did;

        let should_process = {
            let dids_guard: RwLockReadGuard<HashSet<String>> =
                ctx.app_state.active_bsky_dids.read().await;
            dids_guard.contains(&did)
        };

        if !should_process {
            return Ok(());
        }

        if let Some(commit_data) = message.commit {
            if commit_data.operation == "create" && commit_data.collection == "app.bsky.feed.post" {
                if let Some(post_data) = commit_data.record {
                    if post_data.record_type == "app.bsky.feed.post" {
                        if let Err(e) =
                            handle_post_creation(ctx, &did, &commit_data.rkey, post_data).await
                        {
                            println!("[Bluesky Task] Error in post creation for {}: {}", did, e);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_post_creation(
    ctx: &Arc<TaskContext>,
    did: &str,
    rkey: &str,
    post_data: JetstreamPostData,
) -> anyhow::Result<()> {
    let reg = match models::get_bluesky_reg_by_did(&ctx.app_state.db, did).await? {
        Some(r) => r,
        None => {
            println!(
                "[Bluesky Task] Post for DID {} but reg missing (unexpected).",
                did
            );
            return Ok(());
        }
    };

    let display_name_or_handle =
        get_bsky_profile_display_name(&ctx.app_state.http, did, &reg.bsky_handle).await;

    println!(
        "[Bluesky Task] New post found via Jetstream from: {} (Handle: {})",
        display_name_or_handle, reg.bsky_handle
    );

    let post_url = format!("https://bsky.app/profile/{}/post/{}", reg.bsky_handle, rkey);

    let message_text = format!(
        "**New Bluesky Post from {}:**\n\n> {}\n\n{}",
        display_name_or_handle,
        post_data.text.replace('\n', "\n> "),
        post_url
    );

    let max_len = 2000;
    let content = if message_text.len() > max_len {
        format!("{}...", &message_text[..(max_len - 3)])
    } else {
        message_text
    };

    let discord_channel = serenity::model::id::ChannelId::new(reg.discord_channel_id);
    let builder = CreateMessage::new().content(content);

    if let Err(e) = discord_channel
        .send_message(ctx.http.as_ref(), builder)
        .await
    {
        println!(
            "[Bluesky Task] Failed Discord send for {}: {}",
            reg.bsky_handle, e
        );
        return Err(e.into());
    }

    Ok(())
}
