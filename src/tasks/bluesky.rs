use crate::models;
use crate::tasks::TaskContext;

use chrono::{DateTime, Utc};
use dotenvy_macro::dotenv;
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use reqwest::{header, Client as ReqwestClient, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLockReadGuard};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use twilight_http::request::Request;
use twilight_http::routing::Route;
use twilight_model::channel::message::component::{
    ActionRow, Button, ButtonStyle, Container, MediaGallery, MediaGalleryItem, Section,
    TextDisplay, Thumbnail, UnfurledMediaItem,
};
use twilight_model::channel::message::{Component, MessageFlags};
use twilight_model::channel::Message;
use url::Url;

const JETSTREAM_BASE_URL: &str = "wss://jetstream1.us-east.bsky.network/subscribe";
const BLUESKY_CDN_URL: &str = "https://cdn.bsky.app/img/feed_fullsize/plain";

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct JetstreamMessage {
    did: String,
    kind: String,
    commit: Option<JetstreamCommitData>,
    #[serde(flatten)]
    extra: Value,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct JetstreamCommitData {
    operation: String,
    collection: String,
    rkey: String,
    record: Option<JetstreamPostData>,
    #[serde(flatten)]
    extra: Value,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct JetstreamPostData {
    #[serde(rename = "$type")]
    record_type: String,
    text: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    embed: Option<EmbedData>,
    reply: Option<Value>,
    facets: Option<Vec<Facet>>,
    #[serde(flatten)]
    extra: Value,
}

#[derive(Deserialize, Serialize, Debug)]
struct ExternalEmbedData {
    thumb: Option<BlobRef>,
    uri: String,
    title: String,
    description: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct EmbedData {
    #[serde(rename = "$type")]
    embed_type: Option<String>,
    images: Option<Vec<RawImage>>,
    external: Option<ExternalEmbedData>,
    #[serde(flatten)]
    extra: Value,
}

#[derive(Deserialize, Serialize, Debug)]
struct RawImage {
    alt: Option<String>,
    image: Option<BlobRef>,
}

#[derive(Deserialize, Serialize, Debug)]
struct BlobRef {
    #[serde(rename = "ref")]
    cid_link: CidLink,
    #[serde(rename = "mimeType")]
    mime_type: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct CidLink {
    #[serde(rename = "$link")]
    link: String,
}

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

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct BlueskyProfile {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    handle: String,
    avatar: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Facet {
    index: FacetIndex,
    features: Vec<FacetFeature>,
}

#[derive(Deserialize, Debug)]
struct FacetIndex {
    #[serde(rename = "byteStart")]
    byte_start: usize,
    #[serde(rename = "byteEnd")]
    byte_end: usize,
}

#[derive(Deserialize, Debug)]
struct FacetFeature {
    #[serde(rename = "$type")]
    feature_type: String,
    uri: String,
}

static BOT_AUTH_TOKEN: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

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
        println!(
            "[Bluesky Auth] Bot login failed: Status {}, Body: {}",
            status, raw_body
        );
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

async fn get_bsky_profile_data(
    http_client: &ReqwestClient,
    did: &str,
    fallback_handle: &str,
) -> (String, Option<String>) {
    let url = format!(
        "https://bsky.social/xrpc/app.bsky.actor.getProfile?actor={}",
        did
    );
    let token = match get_bot_auth_token(http_client).await {
        Ok(t) => t,
        Err(e) => {
            println!(
                "[Bluesky Profile] Failed to get auth token: {}. Falling back.",
                e
            );
            return (fallback_handle.to_string(), None);
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
                    Ok(profile) => {
                        let name = profile
                            .display_name
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| fallback_handle.to_string());
                        (name, profile.avatar)
                    }
                    Err(e) => {
                        println!("[Bluesky Profile] Parse error: {}", e);
                        (fallback_handle.to_string(), None)
                    }
                }
            } else if status == StatusCode::UNAUTHORIZED {
                println!("[Bluesky Profile] Bot token invalid (401). Clearing cache.");
                *BOT_AUTH_TOKEN.lock().await = None;
                (fallback_handle.to_string(), None)
            } else {
                println!("[Bluesky Profile] Fetch error: Status {}", status);
                (fallback_handle.to_string(), None)
            }
        }
        Err(e) => {
            println!("[Bluesky Profile] Network error: {}", e);
            (fallback_handle.to_string(), None)
        }
    }
}

pub async fn run_bluesky_firehose_loop(ctx: Arc<TaskContext>) {
    loop {
        println!("[Bluesky Task] Fetching registered DIDs...");
        let registrations = match models::get_all_bluesky_registrations(&ctx.app_state.db).await {
            Ok(regs) => regs,
            Err(e) => {
                println!(
                    "[Bluesky Task] CRITICAL ERROR fetching registrations: {:?}. Retrying in 60s.",
                    e
                );
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };

        if registrations.is_empty() {
            println!("[Bluesky Task] No Bluesky DIDs registered. Sleeping for 5 minutes.");
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
            continue;
        }

        let mut url = Url::parse(JETSTREAM_BASE_URL).expect("Failed to parse base Jetstream URL");
        {
            let mut query_pairs = url.query_pairs_mut();
            query_pairs.append_pair("wantedCollections", "app.bsky.feed.post");
            for did in registrations.iter().map(|r| &r.bsky_did) {
                query_pairs.append_pair("wantedDids", did);
            }
        }
        let final_url = url.to_string();
        println!(
            "[Bluesky Task] Connecting to Jetstream with {} DIDs: {}",
            registrations.len(),
            final_url
        );

        match connect_and_listen_jetstream(&ctx, &final_url).await {
            Ok(_) => println!("[Bluesky Task] Listener exited cleanly (unexpected)."),
            Err(e) => println!("[Bluesky Task] Connection error: {}. Reconnecting...", e),
        }

        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn connect_and_listen_jetstream(ctx: &Arc<TaskContext>, url: &str) -> anyhow::Result<()> {
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("User-Agent", "MyRustBot/1.0 (Bluesky Firehose)".parse()?);
    let (ws_stream, _) = connect_async(request).await?;
    println!("[Bluesky Task] Successfully connected.");
    let (_write, mut read) = ws_stream.split();
    while let Some(message_result) = read.next().await {
        match message_result {
            Ok(ws_message) => match ws_message {
                WsMessage::Text(text_data) => {
                    if let Err(e) = handle_jetstream_message(ctx, &text_data).await {
                        println!("[Bluesky Task] Error handling message content: {}", e);
                    }
                }
                WsMessage::Close(e) => {
                    println!("[Bluesky Task] Connection closed: {:?}", e);
                    return Err(anyhow::anyhow!("Connection closed by server"));
                }
                _ => {}
            },
            Err(e) => {
                println!("[Bluesky Task] WebSocket read error: {}", e);
                return Err(e.into());
            }
        }
    }
    Err(anyhow::anyhow!("WebSocket stream ended unexpectedly"))
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct DidOnlyFrame {
    did: Option<String>,
    repo: Option<String>,
    kind: Option<String>,
}

async fn handle_jetstream_message(ctx: &Arc<TaskContext>, data: &str) -> anyhow::Result<()> {
    let did_check: DidOnlyFrame = match serde_json::from_str(data) {
        Ok(frame) => frame,
        Err(_) => return Ok(()),
    };
    let Some(did) = did_check.repo.or(did_check.did) else {
        return Ok(());
    };

    let should_process = {
        let dids_guard: RwLockReadGuard<HashSet<String>> =
            ctx.app_state.active_bsky_dids.read().await;
        dids_guard.contains(&did)
    };
    if !should_process {
        return Ok(());
    }

    let message: JetstreamMessage = match serde_json::from_str(data) {
        Ok(msg) => msg,
        Err(e) => {
            println!(
                "[Bluesky Task] Failed full parse for tracked DID {}: {}",
                did, e
            );
            return Ok(());
        }
    };

    if message.kind == "commit" {
        if let Some(commit_data) = message.commit {
            if commit_data.operation == "create" && commit_data.collection == "app.bsky.feed.post" {
                if let Some(post_data) = commit_data.record {
                    if post_data.record_type == "app.bsky.feed.post" {
                        if post_data.reply.is_none() || post_data.reply == Some(Value::Null) {
                            if let Err(e) =
                                handle_post_creation(ctx, &did, &commit_data.rkey, post_data).await
                            {
                                println!(
                                    "[Bluesky Task] Error in post creation for {}: {}",
                                    did, e
                                );
                            }
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
        None => return Ok(()),
    };

    let (display_name, avatar_url_opt) =
        get_bsky_profile_data(&ctx.app_state.http, did, &reg.bsky_handle).await;

    println!(
        "[Bluesky Task] New post found from: {} (Handle: {})",
        display_name, reg.bsky_handle
    );

    let post_url = format!("https://bsky.app/profile/{}/post/{}", reg.bsky_handle, rkey);
    let profile_url = format!("https://bsky.app/profile/{}", reg.bsky_handle);
    let post_timestamp = DateTime::parse_from_rfc3339(&post_data.created_at)
        .map(|dt| dt.timestamp())
        .unwrap_or(Utc::now().timestamp());

    let header_text_content = format!(
        "### **[{} (@{})]({})**",
        display_name, reg.bsky_handle, profile_url
    );
    let header_text = Component::TextDisplay(TextDisplay {
        content: header_text_content.clone(),
        id: None,
    });

    let post_text = if post_data.text.len() > 3900 {
        format!("{}...", &post_data.text[..3900])
    } else {
        post_data.text.clone()
    };

    let format_body_text = |text: &str, facets: &Option<Vec<Facet>>| -> String {
        let Some(facets) = facets else {
            return text.to_string();
        };

        let mut final_text = String::new();
        let mut last_byte_end = 0;

        let text_bytes = text.as_bytes();

        for facet in facets {
            if let Some(link) = facet
                .features
                .iter()
                .find(|f| f.feature_type == "app.bsky.richtext.facet#link")
            {
                if facet.index.byte_start > last_byte_end {
                    if let Ok(s) =
                        std::str::from_utf8(&text_bytes[last_byte_end..facet.index.byte_start])
                    {
                        final_text.push_str(s);
                    }
                }

                if let Ok(link_text) =
                    std::str::from_utf8(&text_bytes[facet.index.byte_start..facet.index.byte_end])
                {
                    final_text.push_str(&format!("[{}]({})", link_text, link.uri));
                }

                last_byte_end = facet.index.byte_end;
            }
        }

        if last_byte_end < text_bytes.len() {
            if let Ok(s) = std::str::from_utf8(&text_bytes[last_byte_end..]) {
                final_text.push_str(s);
            }
        }

        final_text
    };

    let formatted_text = format_body_text(&post_text, &post_data.facets);

    let body_text_opt = if !post_data.text.is_empty() {
        Some(Component::TextDisplay(TextDisplay {
            content: formatted_text.clone(),
            id: None,
        }))
    } else {
        None
    };

    let accessory_opt = avatar_url_opt.filter(|url| !url.is_empty()).map(|url| {
        Box::new(Component::Thumbnail(Thumbnail {
            description: None,
            id: None,
            media: UnfurledMediaItem {
                url: url,
                height: None,
                content_type: None,
                proxy_url: None,
                width: None,
            },
            spoiler: Some(false),
        }))
    });

    let main_content_component = if let Some(accessory) = accessory_opt {
        let mut section_components = vec![header_text];

        if let Some(body_text) = body_text_opt {
            section_components.push(body_text)
        }

        Component::Section(Section {
            id: None,
            components: section_components,
            accessory: accessory,
        })
    } else {
        let fallback_text = Component::TextDisplay(TextDisplay {
            content: format!("{}\n\n{}", header_text_content, post_text),
            id: None,
        });
        fallback_text
    };

    let media_attachment = post_data
        .embed
        .as_ref()
        .and_then(|embed| build_media_attachment(embed, did));

    let footer_text = Component::TextDisplay(TextDisplay {
        content: format!(
            "-# <:bsky:1431781135746207809> Bluesky • <t:{}:f>",
            post_timestamp
        ),
        id: None,
    });

    let mut container_components = vec![main_content_component];
    if let Some(gallery) = media_attachment {
        container_components.push(gallery);
    }

    container_components.push(footer_text);

    let container = Component::Container(Container {
        id: None,
        accent_color: Some(None),
        components: container_components,
        spoiler: Some(false),
    });

    let button = Component::Button(Button {
        sku_id: None,
        id: None,
        custom_id: None,
        disabled: false,
        emoji: None,
        label: Some("View Post".to_string()),
        style: ButtonStyle::Link,
        url: Some(post_url),
    });
    let action_row = Component::ActionRow(ActionRow {
        id: None,
        components: vec![button],
    });

    let all_components = vec![container, action_row];

    let channel_id_u64 = reg.discord_channel_id;

    let body = json!({
        "components": all_components,
        "flags": MessageFlags::IS_COMPONENTS_V2.bits()
    });

    let body_bytes = serde_json::to_vec(&body)?;

    let route = Route::CreateMessage {
        channel_id: channel_id_u64,
    };

    let request = Request::builder(&route).body(body_bytes).build()?;

    if let Err(e) = ctx.client.request::<Message>(request).await {
        println!(
            "[Bluesky Task] Failed Discord send (Raw) for {}: {}",
            reg.bsky_handle, e
        );
        return Err(e.into());
    }

    Ok(())
}

fn build_media_attachment(embed: &EmbedData, did: &str) -> Option<Component> {
    if embed.embed_type == Some("app.bsky.embed.images".to_string()) {
        let images = embed.images.as_ref()?;

        let gallery_items: Vec<MediaGalleryItem> = images
            .iter()
            .filter_map(|raw_image| {
                raw_image
                    .image
                    .as_ref()
                    .map(|blob| (blob, raw_image.alt.clone()))
            })
            .map(|(blob, alt)| {
                let cid = &blob.cid_link.link;
                let image_url = format!("{}/{}/{}", BLUESKY_CDN_URL, did, cid);
                let description = alt.filter(|alt_text| !alt_text.is_empty());

                MediaGalleryItem {
                    description: description,
                    spoiler: Some(false),
                    media: UnfurledMediaItem {
                        url: image_url,
                        proxy_url: None,
                        height: None,
                        width: None,
                        content_type: None,
                    },
                }
            })
            .collect();

        if gallery_items.is_empty() {
            None
        } else {
            Some(Component::MediaGallery(MediaGallery {
                id: None,
                items: gallery_items,
            }))
        }
    } else if embed.embed_type == Some("app.bsky.embed.external".to_string()) {
        let external = embed.external.as_ref()?;
        let is_animated_gif =
            external.uri.contains("giphy.com") || external.uri.contains("tenor.com");

        let image_url = if is_animated_gif {
            Some(external.uri.clone())
        } else {
            let thumb = external.thumb.as_ref()?;
            let cid = &thumb.cid_link.link;
            Some(format!("{}/{}/{}", BLUESKY_CDN_URL, did, cid))
        }?;

        let description =
            Some(format!("{} - {}", external.title, external.description)).filter(|d| d.len() > 3);

        let gallery_item = MediaGalleryItem {
            description: description,
            spoiler: Some(false),
            media: UnfurledMediaItem {
                url: image_url,
                proxy_url: None,
                height: None,
                width: None,
                content_type: None,
            },
        };
        Some(Component::MediaGallery(MediaGallery {
            id: None,
            items: vec![gallery_item],
        }))
    } else {
        None
    }
}
