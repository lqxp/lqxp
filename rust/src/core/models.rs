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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChannelKind {
    #[serde(rename = "text")]
    #[default]
    Text,
    #[serde(rename = "announce")]
    Announce,
    #[serde(rename = "voice")]
    Voice,
}

impl ChannelKind {
    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" | "chat" | "community" => Some(ChannelKind::Text),
            "announce" | "announcement" | "annonce" | "annonces" => Some(ChannelKind::Announce),
            "voice" | "vocal" | "vocaux" | "audio" => Some(ChannelKind::Voice),
            _ => None,
        }
    }
}

/// Nom de salon façon Discord : minuscules, 2..100, `[a-z0-9-_]` (+ espaces
/// convertis en `-` côté normalisation).
pub fn normalize_channel_name(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.trim().to_ascii_lowercase().chars() {
        if ch == ' ' || ch == '_' {
            out.push('-');
        } else if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
        }
        // tout le reste est ignoré (accents, emojis, ponctuation…)
        if out.len() >= 100 {
            break;
        }
    }
    // Collapse les tirets répétés + trim.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_dash = false;
    for ch in out.chars() {
        if ch == '-' {
            if prev_dash {
                continue;
            }
            prev_dash = true;
        } else {
            prev_dash = false;
        }
        collapsed.push(ch);
    }
    collapsed.trim_matches('-').to_owned()
}

pub fn validate_channel_name(raw: &str) -> Result<String, &'static str> {
    let name = normalize_channel_name(raw);
    let len = name.chars().count();
    if len < 2 {
        return Err("Channel name must be at least 2 characters (a-z, 0-9, -, _)");
    }
    if len > 100 {
        return Err("Channel name must be at most 100 characters");
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("Channel name must match Discord rules (a-z, 0-9, -, _)");
    }
    Ok(name)
}

pub fn validate_category_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim().to_owned();
    let len = name.chars().count();
    if len < 2 {
        return Err("Category name must be at least 2 characters");
    }
    if len > 64 {
        return Err("Category name must be at most 64 characters");
    }
    Ok(name)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerChannel {
    #[serde(default, rename = "id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "kind")]
    pub kind: ChannelKind,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "categoryId")]
    pub category_id: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub topic: String,
    #[serde(default)]
    pub position: i64,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "createdBy")]
    pub created_by: String,
    #[serde(default, rename = "createdAt")]
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCategory {
    #[serde(default, rename = "id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub position: i64,
}

pub fn default_community_channels(owner_id: &str, now: u64) -> (Vec<ChannelCategory>, Vec<ServerChannel>) {
    // Un serveur sans salon démarre avec un unique #news en mode annonce :
    // seuls les admins / sous-admins peuvent y parler.
    let categories = Vec::new();
    let channels = vec![ServerChannel {
        id: "news".to_owned(),
        name: "news".to_owned(),
        kind: ChannelKind::Announce,
        category_id: None,
        topic: String::new(),
        position: 0,
        created_by: owner_id.to_owned(),
        created_at: now,
    }];
    (categories, channels)
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
    #[serde(default, rename = "channels")]
    pub channels: Vec<ServerChannel>,
    #[serde(default, rename = "categories")]
    pub categories: Vec<ChannelCategory>,
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
            channels: Vec::new(),
            categories: Vec::new(),
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
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "channelId")]
    pub channel_id: Option<String>,
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

// ── QXP-PHANTOM (rendez-vous fantôme) ────────────────────────────────────────
// Couche externe d'une enveloppe : les seuls champs visibles par le serveur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhantomEnvelope {
    pub pv: u8,
    pub slot_id: String,
    pub recipient_fp: String,
    pub sender_hint: String,
    pub bucket: u32,
    pub ct: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhantomGateMode {
    Pass,
    Cap,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhantomGate {
    pub mode: PhantomGateMode,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub nullifier: String,
    /// Jeton de quota RLN (obtenu via `GET /api/auth/challenge`).
    #[serde(default)]
    pub quota_token: Option<crate::core::rln::EpochQuotaToken>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhantomDepositRequest {
    pub envelope: PhantomEnvelope,
    pub gate: PhantomGate,
}

fn default_want() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhantomPollRequest {
    #[serde(default)]
    pub slots: Vec<String>,
    #[serde(default = "default_want")]
    pub want: usize,
}

// Bundle de prékey publique (§2.1), persistée telle quelle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrekeyBundle {
    pub version: u32,
    pub mlkem768_pk: String,
    pub ecdsa_p256_pk: serde_json::Value,
    pub mldsa65_pk: String,
    pub sig_ecdsa: String,
    pub sig_mldsa: String,
    #[serde(default)]
    pub block_filter: Vec<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialBlobPutRequest {
    pub ver: i64,
    pub blob: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PassRedeemRequest {
    pub token_response: String,
    pub nonce: String,
}
