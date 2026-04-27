use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub ip: String,
    pub reason: String,
    pub timestamp: u64,
    pub ign: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedIpEntry {
    pub ip: String,
    pub username: String,
    pub version: String,
    #[serde(rename = "isVoiceChat")]
    pub is_voice_chat: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerStatus {
    pub username: String,
    pub ip: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileImage {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "dataB64")]
    pub data_b64: String,
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
    pub filename: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub size: u64,
    #[serde(rename = "dataB64")]
    pub data_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub v: u8,
    pub alg: String,
    pub iv: String,
    pub ciphertext: String,
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
    pub text: String,
    pub timestamp: u64,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct SocketPayload {
    pub op: u16,
    #[serde(default)]
    pub d: serde_json::Value,
    #[serde(default)]
    pub u: Option<String>,
}
