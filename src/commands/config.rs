use crate::{models, AppState};
use serenity::all::{
    CommandDataOption, CommandDataOptionValue, CommandInteraction, CreateInteractionResponse,
    CreateInteractionResponseMessage, Permissions,
};
use serenity::client::Context;
use std::sync::Arc;

pub async fn run(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    command
        .guild_id
        .expect("Configuration commands must be run in a guild.");

    if !command.member.as_ref().map_or(false, |m| {
        m.permissions
            .map_or(false, |p| p.contains(Permissions::MANAGE_GUILD))
    }) {
        let response = CreateInteractionResponseMessage::new()
            .content(
                "🚫 You must have the `Manage Server` permission to use configuration commands.",
            )
            .ephemeral(true);
        command
            .create_response(&ctx.http, CreateInteractionResponse::Message(response))
            .await?;
        return Ok(());
    }

    let option = command.data.options.first().expect("Subcommand missing");

    let options_slice = if let CommandDataOptionValue::SubCommand(options) = &option.value {
        options
    } else {
        return Err(anyhow::anyhow!(
            "Invalid command structure: Expected subcommand value."
        ));
    };

    match option.name.as_str() {
        "channel" => handle_channel_config(ctx, command, app_state, &options_slice).await,
        _ => {
            let response = CreateInteractionResponseMessage::new()
                .content("Unknown configuration subcommand.")
                .ephemeral(true);
            command
                .create_response(&ctx.http, CreateInteractionResponse::Message(response))
                .await?;
            Ok(())
        }
    }
}

async fn handle_channel_config(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
    options: &[CommandDataOption],
) -> anyhow::Result<()> {
    let channel_id_option = options.first().expect("Target channel option missing");

    let target_channel_id = match channel_id_option.value {
        CommandDataOptionValue::Channel(channel_id) => channel_id.get(),
        _ => return Err(anyhow::anyhow!("Invalid channel value provided")),
    };

    let guild_id = command.guild_id.expect("Command must be run in a guild.");

    let config = models::ServerConfig {
        pk: format!("GUILD#{}", guild_id.get()),
        sk: "CONFIG#SETTINGS".to_string(),
        gsi1pk: None, // Not used
        gsi1sk: None, // Not used
        notification_channel_id: target_channel_id,
        item_type: "GUILD_CONFIG".to_string(),
    };

    models::save_server_config(&app_state.db, config).await?;

    let response = CreateInteractionResponseMessage::new()
        .content(format!(
            "✅ Successfully set the notification channel for this server to <#{}>.",
            target_channel_id
        ))
        .ephemeral(true);
    command
        .create_response(&ctx.http, CreateInteractionResponse::Message(response))
        .await?;

    Ok(())
}
