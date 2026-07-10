use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::extract::ws::Message;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::{
    accounts::SharedAccounts,
    config::Config,
    db::RoomDatabase,
    models::{ChatMessageRecord, UserPresenceStatus, UserProfile},
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub blocklist_terms: Arc<Vec<String>>,
    pub players: Arc<RwLock<HashMap<String, PlayerSession>>>,
    pub room_messages: Arc<RwLock<HashMap<String, Vec<ChatMessageRecord>>>>,
    pub database: Arc<RoomDatabase>,
    pub accounts: SharedAccounts,
    pub rate_limits: Arc<Mutex<HashMap<String, RateLimitBucket>>>,
}

pub type SharedState = Arc<AppState>;

#[derive(Debug, Clone)]
pub struct RateLimitBucket {
    pub window_start_ms: u64,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct PlayerSession {
    pub id: String,
    pub user_id: String,
    pub is_admin: bool,
    pub badges: Vec<String>,
    pub username: String,
    pub tx: mpsc::UnboundedSender<Message>,
    pub rooms: HashSet<String>,
    pub is_voice_chat: bool,
    pub call_camera: bool,
    pub call_screen: bool,
    pub client_id: String,
    pub platform: String,
    pub version: String,
    pub last_message_timestamp: Option<u64>,
    pub last_voice_chunk_timestamp: Option<u64>,
    pub is_mobile: Option<bool>,
    pub is_secure: Option<bool>,
    pub muted_users: HashSet<String>,
    pub delete_messages_on_leave: bool,
    pub profile: UserProfile,
    pub status: UserPresenceStatus,
}
