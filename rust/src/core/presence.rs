use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use axum::extract::ws::Message;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::core::{
    config::Config,
    database::{AccountDatabase, RoomDatabase},
    models::{ChatMessageRecord, UserPresenceStatus, UserProfile},
};

/// Counters kept only in memory, for the life of the process.
///
/// Three integers with no identity, no timestamp and no content attached to
/// any of them, gone when the process restarts. They answer "how busy is this
/// server" without recording anything about anybody, which is where the
/// zero-log rule draws its line: the server may count, it may not remember.
#[derive(Debug, Default)]
pub struct RuntimeCounters {
    messages_relayed: AtomicU64,
    sessions_opened: AtomicU64,
    peak_sessions: AtomicU64,
}

impl RuntimeCounters {
    pub fn record_message(&self) {
        self.messages_relayed.fetch_add(1, Ordering::Relaxed);
    }

    /// `live` is the number of sessions held once this one was added.
    pub fn record_session_opened(&self, live: usize) {
        self.sessions_opened.fetch_add(1, Ordering::Relaxed);
        self.peak_sessions.fetch_max(live as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.messages_relayed.load(Ordering::Relaxed),
            self.sessions_opened.load(Ordering::Relaxed),
            self.peak_sessions.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub config: Config,
    pub started_at_ms: u64,
    pub runtime: Arc<RuntimeCounters>,
    pub blocklist_terms: Arc<Vec<String>>,
    pub players: Arc<RwLock<HashMap<String, PlayerSession>>>,
    pub room_messages: Arc<RwLock<HashMap<String, Vec<ChatMessageRecord>>>>,
    pub database: Arc<RoomDatabase>,
    pub accounts: Arc<AccountDatabase>,
    pub rate_limits: Arc<Mutex<HashMap<String, RateLimitBucket>>>,
    pub public_profile_cache: Arc<Mutex<HashMap<String, CachedPublicProfile>>>,
    pub call_access_overrides: Arc<RwLock<HashSet<String>>>,
    pub poll_tallies: Arc<Mutex<HashMap<String, PollTally>>>,
}

/// Anonymous poll tally: salted voter hashes stop a second vote, counts carry the result,
/// and nothing links a voter to a choice. Memory only.
#[derive(Debug, Default, Clone)]
pub struct PollTally {
    pub voters: HashSet<String>,
    pub counts: Vec<u32>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub async fn evict_user(self: &Arc<Self>, user_id: &str, reason: &str) {
        let sessions: Vec<(String, mpsc::Sender<Message>)> = {
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
            let _ = tx.try_send(Message::Text(payload.clone()));
            let _ = tx.try_send(Message::Close(None));
            crate::websocket::disconnect_player(self, &session_id).await;
        }
    }

    pub async fn disconnect_user_sessions(self: &Arc<Self>, user_id: &str, reason: &str) {
        let sessions: Vec<(String, mpsc::Sender<Message>)> = {
            let players = self.players.read().await;
            players
                .values()
                .filter(|player| player.user_id == user_id)
                .map(|player| (player.id.clone(), player.tx.clone()))
                .collect()
        };

        let payload = json!({
            "op": 0,
            "d": { "error": reason }
        })
        .to_string();
        for (session_id, tx) in sessions {
            let _ = tx.try_send(Message::Text(payload.clone()));
            let _ = tx.try_send(Message::Close(None));
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
    pub tx: mpsc::Sender<Message>,
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
    pub identified_at_ms: u64,
    pub last_revalidation_ms: u64,
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
