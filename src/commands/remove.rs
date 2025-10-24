use crate::{models, AppState};
use serenity::all::{
    CommandDataOption, CommandDataOptionValue, CommandInteraction, CreateInteractionResponse,
    CreateInteractionResponseMessage, Permissions,
};
use serenity::client::Context;
use std::sync::Arc;

macro_rules! check_permissions {
    ($ctx:expr, $command:expr) => {{
        let required_permissions = Permissions::MANAGE_GUILD;
        if let Some(member) = &$command.member {
            if !member.permissions.map_or(false, |p| p.contains(required_permissions)) {
                 let response = CreateInteractionResponseMessage::new()
                    .content("🚫 You must have the `Manage Server` permission to remove registrations.")
                    .ephemeral(true);
                $command.create_response(&$ctx.http, CreateInteractionResponse::Message(response)).await?;
                return Ok(());
            }
        } else {
             let response = CreateInteractionResponseMessage::new()
                    .content("🚫 This command is restricted to server administrators.")
                    .ephemeral(true);

                $command.create_response(&$ctx.http, CreateInteractionResponse::Message(response)).await?;

                return Ok(());
        }
    }};
}

pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    check_permissions!(ctx, command);

    let option = command
        .data
        .options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing subcommand"))?;

    let options_slice = if let CommandDataOptionValue::SubCommand(options) = &option.value {
        options
    } else {
        return Err(anyhow::anyhow!(
            "Invalid command structure: Expected subcommand arguments"
        ));
    };

    match option.name.as_str() {
        "rss" => handle_rss_remove(ctx, command, app_state, options_slice).await?,
        "twitch" => handle_twitch_remove(ctx, command, app_state, options_slice).await?,
        "bluesky" => handle_bluesky_remove(ctx, command, app_state, options_slice).await?,
        _ => {
            let response = CreateInteractionResponseMessage::new()
                .content("Unknown subcommand used for /remove.")
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
    }

    Ok(())
}

pub async fn handle_rss_remove(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let url_option = options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing URL option"))?;
    let url = match &url_option.value {
        CommandDataOptionValue::String(url_str) => url_str,
        _ => return Err(anyhow::anyhow!("Invalid URL value type")),
    };

    let pk = format!("CHANNEL#{}", command.channel_id.get());
    let sk = format!("FEED#{}", url);

    match models::delete_registration(&app_state.db, &pk, &sk).await {
        Ok(_) => {
            let response = CreateInteractionResponseMessage::new()
                .content(format!("✅ Successfully removed RSS feed: `{}`", url))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
        Err(e) => {
            println!(
                "Error deleting RSS registration (PK: {}, SK: {}): {:?}",
                pk, sk, e
            );
            let response = CreateInteractionResponseMessage::new()
                .content(format!(
                    "❌ Error removing RSS feed: `{}`. It might have already been removed.",
                    url
                ))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
    }
    Ok(())
}

pub async fn handle_twitch_remove(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let login_option = options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing login option"))?;
    let login = match &login_option.value {
        CommandDataOptionValue::String(login_str) => login_str,
        _ => return Err(anyhow::anyhow!("Invalid login value type")),
    };

    let reg_result = models::get_twitch_reg_by_login(&app_state.db, login).await;

    match reg_result {
        Ok(Some(reg_item)) => {
            match models::delete_registration(&app_state.db, &reg_item.pk, &reg_item.sk).await {
                Ok(_) => {
                    let response = CreateInteractionResponseMessage::new()
                        .content(format!(
                            "✅ Successfully removed Twitch channel: `{}`",
                            login
                        ))
                        .ephemeral(true);
                    command
                        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                        .await?;
                }
                Err(e) => {
                    println!(
                        "Error deleting Twitch registration (PK: {}, SK: {}): {:?}",
                        reg_item.pk, reg_item.sk, e
                    );
                    let response = CreateInteractionResponseMessage::new()
                        .content(format!(
                            "❌ Error removing Twitch channel: `{}`. Please try again.",
                            login
                        ))
                        .ephemeral(true);
                    command
                        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                        .await?;
                }
            }
        }
        Ok(None) => {
            let response = CreateInteractionResponseMessage::new()
                .content(format!(
                    "❌ Error: Twitch channel `{}` not found in registration.",
                    login
                ))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
        Err(e) => {
            println!("Error looking up Twitch login '{}': {:?}", login, e);
            let response = CreateInteractionResponseMessage::new()
                .content(format!(
                    "❌ Database error looking up Twitch channel: `{}`.",
                    login
                ))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
    }
    Ok(())
}

pub async fn handle_bluesky_remove(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let handle_option = options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing handle option"))?;
    let handle = match &handle_option.value {
        CommandDataOptionValue::String(handle_str) => handle_str,
        _ => return Err(anyhow::anyhow!("Invalid handle value type")),
    };

    let reg_result = models::get_bsky_reg_by_handle(&app_state.db, handle).await;

    match reg_result {
        Ok(Some(reg_item)) => {
            let did_to_remove = reg_item.bsky_did.clone();

            match models::delete_registration(&app_state.db, &reg_item.pk, &reg_item.sk).await {
                Ok(_) => {
                    {
                        let mut dids_guard = app_state.active_bsky_dids.write().await;
                        if dids_guard.remove(&did_to_remove) {
                            println!("Removed {} from active Bluesky DID set.", did_to_remove);
                        } else {
                            println!("Warning: {} was not found in active Bluesky DID set during removal.", did_to_remove);
                        }
                    }
                    let response = CreateInteractionResponseMessage::new()
                        .content(format!(
                            "✅ Successfully removed Bluesky handle: `{}`",
                            handle
                        ))
                        .ephemeral(true);
                    command
                        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                        .await?;
                }
                Err(e) => {
                    println!(
                        "Error deleting Bluesky registration (PK: {}, SK: {}): {:?}",
                        reg_item.pk, reg_item.sk, e
                    );
                    let response = CreateInteractionResponseMessage::new()
                        .content(format!(
                            "❌ Error removing Bluesky handle: `{}`. Please try again.",
                            handle
                        ))
                        .ephemeral(true);
                    command
                        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                        .await?;
                }
            }
        }
        Ok(None) => {
            let response = CreateInteractionResponseMessage::new()
                .content(format!(
                    "❌ Error: Bluesky handle `{}` not found in registration.",
                    handle
                ))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
        Err(e) => {
            println!("Error looking up Bluesky handle '{}': {:?}", handle, e);
            let response = CreateInteractionResponseMessage::new()
                .content(format!(
                    "❌ Database error looking up Bluesky handle: `{}`.",
                    handle
                ))
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
    }
    Ok(())
}
