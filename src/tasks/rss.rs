use crate::tasks::TaskContext;
use crate::{models, models::RssRegistration};
use feed_rs::parser as FeedParser;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use twilight_http::request::Request;
use twilight_http::routing::Route;
use twilight_model::channel::message::component::{
    Container, MediaGallery, MediaGalleryItem, TextDisplay, UnfurledMediaItem,
};
use twilight_model::channel::message::{Component, MessageFlags};
use twilight_model::channel::Message;

pub async fn run_rss_poll_loop(ctx: Arc<TaskContext>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60 * 5));

    loop {
        interval.tick().await;
        println!("[RSS Task] Starting poll cycle...");

        println!("[RSS Task] Attempting to fetch registrations from DynamoDB...");
        let regs = match models::get_all_rss_registrations(&ctx.app_state.db).await {
            Ok(regs) => {
                println!(
                    "[RSS Task] Successfully fetched {} registrations.",
                    regs.len()
                );
                regs
            }
            Err(e) => {
                println!(
                    "[RSS Task] CRITICAL ERROR fetching RSS registrations: {:?}",
                    e
                );
                continue;
            }
        };

        println!("[RSS Task] Checking {} feeds...", regs.len());
        for reg in regs {
            if let Err(e) = check_feed(&ctx, &reg).await {
                println!("[RSS Task] Error checking feed {}: {:?}", reg.feed_url, e);
            }
        }
        println!("[RSS Task] Finished poll cycle.");
    }
}

async fn check_feed(ctx: &Arc<TaskContext>, reg: &RssRegistration) -> anyhow::Result<()> {
    let content = ctx
        .app_state
        .http
        .get(&reg.feed_url)
        .send()
        .await?
        .bytes()
        .await?;

    let feed = FeedParser::parse(std::io::Cursor::new(content))?;

    let latest_entry = match feed.entries.first() {
        Some(entry) => entry,
        None => return Ok(()),
    };

    let latest_id = &latest_entry.id;
    let fallback_link = latest_entry.links.first().map_or("", |link| &link.href);
    let latest_guid_or_link = if !latest_id.is_empty() {
        latest_id.as_str()
    } else {
        fallback_link
    };

    let has_new_post = reg.last_post_guid.as_deref() != Some(latest_guid_or_link);

    if has_new_post && !latest_guid_or_link.is_empty() {
        println!(
            "[RSS Task] New post found for {}: {}",
            reg.feed_url, latest_guid_or_link
        );

        let title_content = latest_entry
            .clone()
            .title
            .or(feed.title)
            .map(|text_obj| text_obj.content)
            .unwrap_or("New post".to_string());

        let link = latest_entry.links.first().map_or("", |l| &l.href);

        let mut is_video = false;

        if let Some(media_content) = latest_entry.media.first().and_then(|m| m.content.first()) {
            if let Some(content_type) = &media_content.content_type {
                if content_type.essence().ty.as_ref() == "video" {
                    is_video = true;
                }
            }
        }
        if !is_video {
            if link.contains("youtube.com")
                || link.contains("youtu.be")
                || link.contains("vimeo.com")
            {
                is_video = true;
            }
        }

        let channel_id = reg.channel_id;
        let route = Route::CreateMessage { channel_id };
        let body_bytes: Vec<u8>;

        if is_video {
            println!(
                "[RSS Task] Video detected. Sending simple message for: {}",
                link
            );

            let message = format!("**New RSS/Atom Post:** {}\n{}", title_content, link);

            let body = json!({
                "content": message
            });

            body_bytes = serde_json::to_vec(&body)?;
        } else {
            println!("[RSS Task] No video. Sending v2 component for: {}", link);

            let post_timestamp = &latest_entry.updated.map(|dt| dt.timestamp()).unwrap();
            let title = format!("### ** [{}]({})**", title_content, link);

            let title_text = Component::TextDisplay(TextDisplay {
                content: title,
                id: None,
            });

            let thumbnail_uri_option = latest_entry
                .media
                .first()
                .and_then(|media| media.thumbnails.first())
                .map(|thumbnail| &thumbnail.image.uri);

            let media_content = if let Some(media) = thumbnail_uri_option {
                let item = MediaGalleryItem {
                    description: None,
                    spoiler: Some(false),
                    media: UnfurledMediaItem {
                        url: media.to_string(),
                        proxy_url: None,
                        height: None,
                        width: None,
                        content_type: None,
                    },
                };

                Some(Component::MediaGallery(MediaGallery {
                    id: None,
                    items: vec![item],
                }))
            } else {
                None
            };

            let mut main_content = vec![title_text];

            if let Some(media) = media_content {
                main_content.push(media)
            };

            let footer_text = Component::TextDisplay(TextDisplay {
                content: format!(
                    "-# <:rss:1431781137545429043> RSS / Atom • <t:{}:f>",
                    post_timestamp
                ),
                id: None,
            });

            main_content.push(footer_text);

            let container = Component::Container(Container {
                id: None,
                accent_color: Some(None),
                components: main_content,
                spoiler: Some(false),
            });

            let body = json!({
                "components": vec![container],
                "flags": MessageFlags::IS_COMPONENTS_V2.bits()
            });

            body_bytes = serde_json::to_vec(&body)?;
        }

        let request = Request::builder(&route).body(body_bytes).build()?;

        if let Err(e) = ctx.client.request::<Message>(request).await {
            println!(
                "[RSS Task] Failed Discord send (Raw) for {}: {}",
                reg.feed_url, e
            );
            return Err(e.into());
        }

        let pk = reg.pk.clone();
        let sk = reg.sk.clone();
        models::update_rss_last_post_guid(&ctx.app_state.db, &pk, &sk, latest_guid_or_link).await?;
    }

    Ok(())
}
