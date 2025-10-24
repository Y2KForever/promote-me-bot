use crate::AppState;
use serenity::http::Http;
use std::sync::Arc;

pub mod bluesky;
pub mod rss;
pub mod twitch;

pub struct TaskContext {
    pub http: Arc<Http>,
    pub app_state: Arc<AppState>,
}
