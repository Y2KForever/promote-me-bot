use dotenvy_macro::dotenv;
use rustls::crypto;
use serenity::all::ActivityData;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::model::application::Interaction;
use serenity::model::gateway::Ready;
use serenity::prelude::GatewayIntents;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use twilight_http::Client as TwilightClient;

mod commands;
mod models;
mod oauth;
mod tasks;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TwitchEventInfo {
    pub broadcaster_user_id: String,
    pub broadcaster_user_login: String,
    pub broadcaster_user_name: String,
    pub title: Option<String>,
    pub category_name: Option<String>,
}

pub struct AppState {
    db: aws_sdk_dynamodb::Client,
    http: reqwest::Client,
    twitch_event_sender: mpsc::Sender<TwitchEventInfo>,
    active_bsky_dids: Arc<RwLock<HashSet<String>>>,
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            let app_state = {
                let data = ctx.data.read().await;
                data.get::<AppStateKey>()
                    .cloned()
                    .expect("AppState not found in TypeMap")
            };

            if let Err(e) = commands::handle_command(&ctx, &command, app_state).await {
                println!("Error handling command '{}': {:?}", command.data.name, e);
                let _ = command
                    .create_response(
                        &ctx.http,
                        serenity::all::CreateInteractionResponse::Message(
                            serenity::all::CreateInteractionResponseMessage::new()
                                .content("An error occurred while processing your command.")
                                .ephemeral(true),
                        ),
                    )
                    .await;
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
        if let Err(e) = commands::register_commands(&ctx.http).await {
            println!("Error registering commands: {:?}", e);
        }
    }
}

struct AppStateKey;
impl serenity::prelude::TypeMapKey for AppStateKey {
    type Value = Arc<AppState>;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    crypto::aws_lc_rs::default_provider()
        .install_default()
        .unwrap_or_default();
    dotenvy::dotenv().ok();
    let discord_token = dotenv!("DISCORD_TOKEN");
    let base_url = dotenv!("BASE_URL");
    let (twitch_tx, twitch_rx) = mpsc::channel::<TwitchEventInfo>(100);

    let initial_dids = {
        let temp_aws_config =
            aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let temp_db_client = aws_sdk_dynamodb::Client::new(&temp_aws_config);
        match models::get_all_bluesky_registrations(&temp_db_client).await {
            Ok(regs) => regs.into_iter().map(|r| r.bsky_did).collect(),
            Err(e) => {
                println!("CRITICAL ERROR: Failed to fetch initial Bluesky DIDs: {}. Starting with empty set.", e);
                HashSet::new()
            }
        }
    };
    println!(
        "Loaded {} initial Bluesky DIDs into memory.",
        initial_dids.len()
    );
    let active_bsky_dids_set = Arc::new(RwLock::new(initial_dids));
    let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let db_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let http_client = reqwest::Client::new();
    let app_state = Arc::new(AppState {
        db: db_client,
        http: http_client,
        twitch_event_sender: twitch_tx,
        active_bsky_dids: active_bsky_dids_set,
    });
    let twilight_http_client = Arc::new(TwilightClient::new(discord_token.to_string()));

    let intents =
        GatewayIntents::GUILDS | GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILD_PRESENCES;
    let mut client = Client::builder(&discord_token, intents)
        .event_handler(Handler)
        .activity(ActivityData::custom("Watching F1 fanfic smut 🏎️💨"))
        .await
        .expect("Error creating Serenity client");

    {
        let mut data = client.data.write().await;
        data.insert::<AppStateKey>(app_state.clone());
    }

    let task_context = Arc::new(tasks::TaskContext {
        http: client.http.clone(),
        app_state: app_state.clone(),
        client: twilight_http_client,
    });

    tokio::spawn(tasks::rss::run_rss_poll_loop(task_context.clone()));
    tokio::spawn(tasks::bluesky::run_bluesky_firehose_loop(
        task_context.clone(),
    ));
    tokio::spawn(tasks::twitch::run_twitch_event_handler(
        task_context.clone(),
        twitch_rx,
    ));

    let web_server_state = oauth::WebServerState {
        app_state: app_state.clone(),
        discord_http: client.http.clone(),
    };
    let server_port = url::Url::parse(&base_url)
        .ok()
        .and_then(|u| u.port())
        .unwrap_or(8080);
    tokio::spawn(oauth::start_server(web_server_state, server_port));

    println!("Starting Discord client...");
    if let Err(e) = client.start().await {
        println!("Client error: {:?}", e);
    }

    Ok(())
}
