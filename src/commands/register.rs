use crate::models::{BlueskyRegistration, RssRegistration};
use crate::{models, AppState};
use serde::Deserialize;
use serenity::all::{
    CommandDataOption, CommandDataOptionValue, CommandInteraction, CreateInteractionResponse,
    CreateInteractionResponseMessage, Permissions,
};
use serenity::client::Context;
use std::env;
use std::sync::Arc;

#[derive(Deserialize)]
struct DidResolution {
    did: String,
}

async fn resolve_notification_channel(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: &Arc<AppState>,
) -> anyhow::Result<Option<u64>> {
    let guild_id = command.guild_id.expect("Command must be run in a guild.");

    match models::get_server_config(&app_state.db, guild_id.get()).await? {
        Some(config) => Ok(Some(config.notification_channel_id)),
        None => {
            let response = CreateInteractionResponseMessage::new()
                .content("🚫 Error: Please have an administrator run `/config channel` first to set the default notification channel for this server.")
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
            Ok(None)
        }
    }
}

pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    let option = command
        .data
        .options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing subcommand"))?;

    let options_slice = if let CommandDataOptionValue::SubCommand(options) = &option.value {
        options
    } else {
        return Err(anyhow::anyhow!(
            "Invalid command structure: Expected subcommand value."
        ));
    };

    match option.name.as_str() {
        "rss" => handle_rss(ctx, command, app_state, options_slice).await?,
        "twitch" => handle_twitch(ctx, command).await?,
        "bluesky" => handle_bluesky(ctx, command, app_state, options_slice).await?,
        _ => {
            let response = CreateInteractionResponseMessage::new()
                .content("Unknown subcommand used for /register.")
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
        }
    }

    Ok(())
}

async fn handle_rss(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let required_permissions = Permissions::MANAGE_GUILD;
    if let Some(member) = &command.member {
        if !member
            .permissions
            .map_or(false, |p| p.contains(required_permissions))
        {
            let response = CreateInteractionResponseMessage::new()
                .content("🚫 You must have the `Manage Server` permission to register RSS feeds.")
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
            return Ok(());
        }
    } else {
        let response = CreateInteractionResponseMessage::new()
            .content("🚫 This command is restricted to server administrators.")
            .ephemeral(true);
        command
            .create_response(&ctx.http, CreateInteractionResponse::Message(response))
            .await?;
        return Ok(());
    }

    let Some(target_channel_id) = resolve_notification_channel(ctx, command, &app_state).await?
    else {
        return Ok(());
    };

    let url_option = options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing URL option"))?;
    let url = match &url_option.value {
        CommandDataOptionValue::String(url_str) => url_str,
        _ => return Err(anyhow::anyhow!("Invalid URL value type")),
    };

    let guild_id = command.guild_id.expect("Command must be run in a guild.");

    let reg = RssRegistration {
        pk: format!("GUILD#{}", guild_id.get()),
        sk: format!("FEED#{}", url),
        gsi1pk: None,
        gsi1sk: None,
        channel_id: target_channel_id,
        feed_url: url.to_string(),
        last_post_guid: None,
        item_type: "RSS_FEED".to_string(),
    };

    models::save_rss_registration(&app_state.db, reg).await?;

    let response = CreateInteractionResponseMessage::new()
        .content(format!(
            "✅ Successfully registered RSS feed: `{}`. Updates will post to <#{}>.",
            url, target_channel_id
        ))
        .ephemeral(true);
    command
        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
        .await?;
    Ok(())
}

async fn handle_twitch(ctx: &Context, command: &CommandInteraction) -> anyhow::Result<()> {
    let base_url = env::var("BASE_URL")?;
    let client_id = env::var("TWITCH_CLIENT_ID")?;

    let guild_id = command.guild_id.expect("Command must be run in a guild.");
    let state = format!("{}_{}", command.user.id.get(), guild_id.get());

    let auth_url = format!(
        "https://id.twitch.tv/oauth2/authorize?client_id={}&redirect_uri={}/auth/twitch/callback&response_type=code&scope=user:read:email&state={}&force_verify=true",
        client_id,
        base_url,
        state
    );

    let response = CreateInteractionResponseMessage::new()
        .content(format!(
            "Please authorize with Twitch to register your channel:\n\n[Click here to authorize]({})",
            auth_url
        ))
        .ephemeral(true);

    command
        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
        .await?;
    Ok(())
}

async fn handle_bluesky(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let required_permissions = Permissions::MANAGE_GUILD;
    if let Some(member) = &command.member {
        if !member
            .permissions
            .map_or(false, |p| p.contains(required_permissions))
        {
            let response = CreateInteractionResponseMessage::new()
                .content(
                    "🚫 You must have the `Manage Server` permission to register Bluesky accounts.",
                )
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
            return Ok(());
        }
    } else {
        let response = CreateInteractionResponseMessage::new()
            .content("🚫 This command is restricted to server administrators.")
            .ephemeral(true);
        command
            .create_response(&ctx.http, CreateInteractionResponse::Message(response))
            .await?;
        return Ok(());
    }

    let Some(target_channel_id) = resolve_notification_channel(ctx, command, &app_state).await?
    else {
        return Ok(());
    };

    let handle_option = options
        .first()
        .ok_or_else(|| anyhow::anyhow!("Missing handle option"))?;
    let handle = match &handle_option.value {
        CommandDataOptionValue::String(handle_str) => handle_str.trim().to_lowercase(),
        _ => return Err(anyhow::anyhow!("Invalid Bluesky handle value")),
    };

    let url = format!(
        "https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle={}",
        handle
    );
    let response = app_state.http.get(&url).send().await?;
    if !response.status().is_success() {
        let error_msg = format!(
            "❌ Could not resolve handle `{}`. Does it exist or is it inactive? Status: {}",
            handle,
            response.status()
        );
        let err_resp = CreateInteractionResponseMessage::new()
            .content(error_msg)
            .ephemeral(true);
        command
            .create_response(&ctx.http, CreateInteractionResponse::Message(err_resp))
            .await?;
        return Ok(());
    }
    let resolution: DidResolution = response.json().await?;
    let bsky_did = resolution.did;
    let guild_id = command.guild_id.expect("Command must be run in a guild.");

    let reg = BlueskyRegistration {
        pk: format!("GUILD#{}", guild_id.get()),
        sk: format!("BSKY#{}", bsky_did),
        gsi1pk: Some("BSKY_LOOKUP".to_string()),
        gsi1sk: Some(format!("HANDLE#{}", handle)),

        discord_channel_id: target_channel_id,
        bsky_did: bsky_did.clone(),
        bsky_handle: handle.clone(),
        access_token: "admin_reg_token".to_string(),
        refresh_token: "admin_reg_refresh".to_string(),
        last_post_uri: None,
        item_type: "BSKY_REG".to_string(),
    };

    models::save_bluesky_registration(&app_state.db, reg).await?;

    {
        let mut dids_guard = app_state.active_bsky_dids.write().await;
        dids_guard.insert(bsky_did.clone());
        println!("Added {} to active Bluesky DID set.", bsky_did);
    }

    let conf_resp = CreateInteractionResponseMessage::new()
        .content(format!(
            "✅ Successfully registered Bluesky account: `{}`. Updates will post to <#{}>.",
            handle, target_channel_id
        ))
        .ephemeral(true);
    command
        .create_response(&ctx.http, CreateInteractionResponse::Message(conf_resp))
        .await?;
    Ok(())
}
