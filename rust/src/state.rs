use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::extract::ws::Message;
use tokio::sync::{mpsc, RwLock};

use crate::{
    config::Config,
    db::JsonDatabase,
    models::{ChatMessageRecord, UserProfile},
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub blocklist_terms: Arc<Vec<String>>,
    pub players: Arc<RwLock<HashMap<String, PlayerSession>>>,
    pub ip_connections: Arc<RwLock<HashMap<String, usize>>>,
    pub room_messages: Arc<RwLock<HashMap<String, Vec<ChatMessageRecord>>>>,
    pub database: Arc<JsonDatabase>,
}

pub type SharedState = Arc<AppState>;

#[derive(Debug, Clone)]
pub struct PlayerSession {
    pub id: String,
    pub ip: String,
    pub username: String,
    pub tx: mpsc::UnboundedSender<Message>,
    pub rooms: HashSet<String>,
    pub is_voice_chat: bool,
    pub call_camera: bool,
    pub call_screen: bool,
    pub version: String,
    pub last_message_timestamp: Option<u64>,
    pub last_voice_chunk_timestamp: Option<u64>,
    pub exchange_key: Option<String>,
    pub is_mobile: Option<bool>,
    pub is_secure: Option<bool>,
    pub muted_users: HashSet<String>,
    pub delete_messages_on_leave: bool,
    pub profile: UserProfile,
}
