use crate::AppState;
use serenity::all::{Command, CommandInteraction, CreateCommand, Permissions};
use serenity::builder::CreateCommandOption;
use serenity::client::Context;
use serenity::http::Http;
use serenity::model::application::CommandOptionType;
use std::sync::Arc;

pub mod config;
pub mod register;
pub mod remove;

pub async fn handle_command(
    ctx: &Context,
    command: &CommandInteraction,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    match command.data.name.as_str() {
        "register" => register::run(ctx, command, app_state).await?,
        "remove" => remove::run(ctx, command, app_state).await?,
        "config" => config::run(ctx, command, app_state).await?,
        _ => {
            println!("Unhandled command: {}", command.data.name);
        }
    }
    Ok(())
}

pub async fn register_commands(http: &Http) -> anyhow::Result<()> {
    Command::create_global_command(
        http,
        CreateCommand::new("register")
            .description("Register a feed or account for promotion")
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "rss",
                    "Register an RSS feed (Admin only)",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "url",
                        "The URL of the RSS feed",
                    )
                    .required(true),
                ),
            )
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "twitch",
                "Register your Twitch channel via OAuth",
            ))
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "bluesky",
                    "Register a Bluesky account (Admin only)",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "handle",
                        "The Bluesky handle (e.g., jay.bsky.social)",
                    )
                    .required(true),
                ),
            ),
    )
    .await?;

    Command::create_global_command(
        http,
        CreateCommand::new("remove")
            .description("Remove an existing feed or account registration (Admin only)")
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "rss",
                    "Remove a registered RSS feed",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "url",
                        "The URL of the RSS feed to remove",
                    )
                    .required(true),
                ),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "twitch",
                    "Remove a registered Twitch channel",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "login",
                        "The Twitch login/username to remove (e.g., ninja)",
                    )
                    .required(true),
                ),
            )
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "bluesky",
                    "Remove a registered Bluesky account",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "handle",
                        "The Bluesky handle to remove (e.g., jay.bsky.social)",
                    )
                    .required(true),
                ),
            ),
    )
    .await?;

    Command::create_global_command(
        http,
        CreateCommand::new("config")
            .description("Server configuration (Admin only)")
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "channel",
                    "Set the default channel for all notifications.",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::Channel,
                        "target",
                        "The channel where RSS/Twitch/Bluesky updates will be posted.",
                    )
                    .required(true),
                ),
            ),
    )
    .await?;

    Ok(())
}
