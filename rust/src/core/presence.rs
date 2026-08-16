use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use axum::extract::ws::Message;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::core::{
    config::Config,
    database::{AccountDatabase, RoomDatabase},
    models::{ChatMessageRecord, UserPresenceStatus, UserProfile},
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub blocklist_terms: Arc<Vec<String>>,
    pub players: Arc<RwLock<HashMap<String, PlayerSession>>>,
    pub room_messages: Arc<RwLock<HashMap<String, Vec<ChatMessageRecord>>>>,
    pub database: Arc<RoomDatabase>,
    pub accounts: Arc<AccountDatabase>,
    pub rate_limits: Arc<Mutex<HashMap<String, RateLimitBucket>>>,
    pub public_profile_cache: Arc<Mutex<HashMap<String, CachedPublicProfile>>>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub async fn evict_user(self: &Arc<Self>, user_id: &str, reason: &str) {
        let sessions: Vec<(String, mpsc::UnboundedSender<Message>)> = {
            let players = self.players.read().await;
            players
                .values()
                .filter(|player| player.user_id == user_id)
                .map(|player| (player.id.clone(), player.tx.clone()))
                .collect()
        };

        let payload = json!({
            "op": 999,
            "d": { "reason": reason }
        })
        .to_string();
        for (session_id, tx) in sessions {
            let _ = tx.send(Message::Text(payload.clone()));
            let _ = tx.send(Message::Close(None));
            crate::websocket::disconnect_player(self, &session_id).await;
        }
    }

    pub async fn invalidate_public_profile_cache(&self, user_id: Option<&str>, username: Option<&str>) {
        let mut cache = self.public_profile_cache.lock().await;
        cache.retain(|key, _| {
            if let Some(id) = user_id {
                if key == &format!("id:{}", id.trim()) {
                    return false;
                }
            }
            if let Some(name) = username {
                if key == &format!("username:{}", name.trim().to_ascii_lowercase()) {
                    return false;
                }
            }
            true
        });
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitBucket {
    pub window_start_ms: u64,
    pub window_ms: u64,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct CachedPublicProfile {
    pub expires_at_ms: u64,
    pub value: Value,
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
    pub call_room: Option<String>,
    pub call_camera: bool,
    pub call_screen: bool,
    pub call_deafened: bool,
    pub client_id: String,
    pub platform: String,
    pub version: String,
    pub last_message_timestamp: Option<u64>,
    pub is_mobile: Option<bool>,
    pub is_secure: Option<bool>,
    pub muted_users: HashSet<String>,
    pub delete_messages_on_leave: bool,
    pub profile: UserProfile,
    pub status: UserPresenceStatus,
}

impl PlayerSession {
    /// Whether this session is currently in a call that is happening in
    /// `game_id`. A legacy session without a known call room (None) is treated
    /// as "in call" everywhere for backward compatibility.
    pub fn is_call_in_room(&self, game_id: &str) -> bool {
        self.is_voice_chat
            && self
                .call_room
                .as_deref()
                .map_or(true, |room| room == game_id)
    }
}
