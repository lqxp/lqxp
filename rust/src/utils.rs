use axum::extract::ws::Message;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

pub use crate::{
    core::security::{rate_limit_hit, sanitize_filename},
    services::user::store_uploaded_bytes,
};

pub fn send_json(tx: &mpsc::UnboundedSender<Message>, payload: Value) {
    let _ = tx.send(Message::Text(payload.to_string()));
}

pub fn request_id(value: &Value) -> Option<String> {
    value
        .get("requestId")
        .and_then(Value::as_str)
        // Plafonné à 128 caractères côté serveur (S5).
        .map(|s| s.chars().take(128).collect())
}

pub fn with_request_id(mut payload: Value, request_id: Option<String>) -> Value {
    if let Some(request_id) = request_id {
        if let Some(d) = payload.get_mut("d").and_then(Value::as_object_mut) {
            d.insert("requestId".to_owned(), Value::String(request_id));
        } else if let Some(root) = payload.as_object_mut() {
            let mut d = Map::new();
            d.insert("requestId".to_owned(), Value::String(request_id));
            root.insert("d".to_owned(), Value::Object(d));
        }
    }

    payload
}

pub fn random_session_id() -> String {
    crate::core::security::generate_session_token()
}

pub fn random_message_id() -> String {
    Uuid::new_v4().simple().to_string()
}
