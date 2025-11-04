use crate::AppState;
use serenity::http::Http;
use std::sync::Arc;
use twilight_http::Client as TwilightClient;

pub mod bluesky;
pub mod rss;
pub mod twitch;
pub mod tiktok;

pub struct TaskContext {
    pub http: Arc<Http>,
    pub app_state: Arc<AppState>,
    pub client: Arc<TwilightClient>,
}
