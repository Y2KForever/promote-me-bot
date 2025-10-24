use crate::tasks::TaskContext;
use crate::{models, models::RssRegistration};
use serenity::builder::CreateMessage;
use serenity::model::id::ChannelId;
use std::sync::Arc;
use std::time::Duration;

pub async fn run_rss_poll_loop(ctx: Arc<TaskContext>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

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
    let channel = rss::Channel::read_from(&content[..])?;

    let latest_item = match channel.items.first() {
        Some(item) => item,
        None => return Ok(()),
    };

    let latest_guid = latest_item.guid.as_ref().map_or_else(
        || latest_item.link.as_deref().unwrap_or_default(),
        |guid| guid.value.as_str(),
    );

    let has_new_post = reg.last_post_guid.as_deref() != Some(latest_guid);

    if has_new_post && !latest_guid.is_empty() {
        println!("New post found for {}: {}", reg.feed_url, latest_guid);

        let title = latest_item.title.as_deref().unwrap_or("New Post");
        let link = latest_item.link.as_deref().unwrap_or_default();
        let message = format!("**New RSS Post:** {}\n{}", title, link);

        let discord_channel = ChannelId::new(reg.channel_id);
        let builder = CreateMessage::new().content(message);

        discord_channel
            .send_message(ctx.http.as_ref(), builder)
            .await?;

        let mut updated_reg = reg.clone();
        updated_reg.last_post_guid = Some(latest_guid.to_string());
        models::save_rss_registration(&ctx.app_state.db, updated_reg).await?;
    }

    Ok(())
}
