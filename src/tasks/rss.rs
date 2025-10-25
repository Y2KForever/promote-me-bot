use crate::tasks::TaskContext;
use crate::{models, models::RssRegistration};
use feed_rs::parser as FeedParser;
use serenity::builder::CreateMessage;
use serenity::model::id::ChannelId;
use std::sync::Arc;
use std::time::Duration;

pub async fn run_rss_poll_loop(ctx: Arc<TaskContext>) {
    let mut interval = tokio::time::interval(Duration::from_secs(15 * 60));

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

        let title = latest_entry
            .title
            .as_ref()
            .map_or("New Post", |t| &t.content);
        let link = latest_entry.links.first().map_or("", |l| &l.href);

        let message = format!("**New RSS/Atom Post:** {}\n{}", title, link);

        let discord_channel = ChannelId::new(reg.channel_id);
        let builder = CreateMessage::new().content(message);
        discord_channel
            .send_message(ctx.http.as_ref(), builder)
            .await?;

        let pk = reg.pk.clone();
        let sk = reg.sk.clone();
        models::update_rss_last_post_guid(&ctx.app_state.db, &pk, &sk, latest_guid_or_link).await?;
    }

    Ok(())
}
