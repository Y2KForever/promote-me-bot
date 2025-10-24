use crate::models;
use crate::tasks::TaskContext;
use crate::TwitchEventInfo;
use serenity::builder::CreateMessage;
use serenity::model::id::ChannelId;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

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

        let message = format!(
            "🔴 **{} is now live on Twitch!**\n\nhttps://twitch.tv/{}",
            event.broadcaster_user_name, event.broadcaster_user_login
        );

        let channel = ChannelId::new(reg.discord_channel_id);
        let builder = CreateMessage::new().content(message);
        if let Err(e) = channel.send_message(ctx.http.as_ref(), builder).await {
            println!(
                "Failed to send Discord message for {}: {}",
                event.broadcaster_user_login, e
            );
        }
    }

    println!("Twitch Event Handler task finished.");
}
