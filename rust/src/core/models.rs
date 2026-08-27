use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerStatus {
    pub username: String,
    pub id: String,
    #[serde(rename = "isVoiceChat")]
    pub is_voice_chat: bool,
    pub rooms: Vec<String>,
    pub version: String,
    pub mobile: Option<bool>,
    #[serde(rename = "secureContext")]
    pub secure_context: Option<bool>,
    #[serde(rename = "deleteMessagesOnLeave")]
    pub delete_messages_on_leave: bool,
    pub profile: UserProfile,
    pub status: UserPresenceStatus,
    pub badges: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub enum UserPresenceStatus {
    #[default]
    Online,
    Invisible,
    #[serde(rename = "dnd")]
    Dnd,
}


pub fn status_to_str(status: UserPresenceStatus) -> &'static str {
    match status {
        UserPresenceStatus::Online => "online",
        UserPresenceStatus::Invisible => "invisible",
        UserPresenceStatus::Dnd => "dnd",
    }
}

pub fn status_from_str(s: &str) -> UserPresenceStatus {
    match s.trim().to_lowercase().as_str() {
        "invisible" => UserPresenceStatus::Invisible,
        "dnd" => UserPresenceStatus::Dnd,
        _ => UserPresenceStatus::Online,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredFile {
    pub id: String,
    pub url: String,
    pub size: u64,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomIcon {
    #[serde(flatten)]
    pub file: StoredFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoomKind {
    #[serde(rename = "classic")]
    #[default]
    Classic,
    #[serde(rename = "community")]
    Community,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum RoomRole {
    #[serde(rename = "member")]
    #[default]
    Member,
    #[serde(rename = "moderator")]
    Moderator,
    #[serde(rename = "subAdmin")]
    SubAdministrator,
    #[serde(rename = "administrator")]
    Administrator,
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModeratorPermissions {
    #[serde(default = "default_true", rename = "canBan")]
    pub can_ban: bool,
    #[serde(default = "default_true", rename = "canKick")]
    pub can_kick: bool,
    #[serde(default = "default_true", rename = "canMute")]
    pub can_mute: bool,
    #[serde(default = "default_true", rename = "canDelete")]
    pub can_delete: bool,
}

impl Default for ModeratorPermissions {
    fn default() -> Self {
        Self {
            can_ban: true,
            can_kick: true,
            can_mute: true,
            can_delete: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRecord {
    #[serde(rename = "roomId")]
    pub room_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "icon")]
    pub icon: Option<RoomIcon>,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, rename = "kind")]
    pub kind: RoomKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ownerId")]
    pub owner_id: Option<String>,
    #[serde(default, rename = "chatLocked")]
    pub chat_locked: bool,
    #[serde(default, rename = "roles")]
    pub roles: BTreeMap<String, RoomRole>,
    #[serde(default, rename = "banned")]
    pub banned: BTreeMap<String, String>,
    #[serde(default, rename = "timeouts")]
    pub timeouts: BTreeMap<String, u64>,
    #[serde(default, rename = "modPermissions")]
    pub mod_permissions: ModeratorPermissions,
    #[serde(default = "default_true", rename = "callsEnabled")]
    pub calls_enabled: bool,
}

impl Default for RoomRecord {
    fn default() -> Self {
        Self {
            room_id: String::new(),
            title: String::new(),
            icon: None,
            members: Vec::new(),
            kind: RoomKind::Classic,
            description: String::new(),
            owner_id: None,
            chat_locked: false,
            roles: BTreeMap::new(),
            banned: BTreeMap::new(),
            timeouts: BTreeMap::new(),
            mod_permissions: ModeratorPermissions::default(),
            calls_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileImage {
    #[serde(flatten)]
    pub file: StoredFile,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<ProfileImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<ProfileImage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pronouns: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReaction {
    pub emoji: String,
    pub users: Vec<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub url: String,
    pub filename: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub v: u8,
    pub alg: String,
    pub iv: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub salt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "senderDeviceId")]
    pub sender_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "senderSigningKey")]
    pub sender_signing_key: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    pub ciphertext: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "roomId")]
    pub room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LinkPreview {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "siteName")]
    pub site_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageRecord {
    #[serde(rename = "messageId")]
    pub message_id: String,
    #[serde(rename = "roomId")]
    pub room_id: String,
    pub user: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "userId")]
    pub user_id: String,
    pub text: String,
    pub timestamp: u64,
    #[serde(default)]
    pub profile: UserProfile,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "editedAt")]
    pub edited_at: Option<u64>,
    pub system: bool,
    pub reactions: Vec<MessageReaction>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "replyToMessageId"
    )]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<EncryptedPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<LinkPreview>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "deletedBy")]
    pub deleted_by: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not", rename = "deletedByModerator")]
    pub deleted_by_moderator: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SocketPayload {
    pub op: u16,
    #[serde(default)]
    pub d: serde_json::Value,
}
