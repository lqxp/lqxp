use std::{net::SocketAddr, time::{SystemTime, UNIX_EPOCH}};

use axum::{extract::ws::Message, http::HeaderMap};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::AppState;

pub fn send_json(tx: &mpsc::UnboundedSender<Message>, payload: Value) {
    let _ = tx.send(Message::Text(payload.to_string().into()));
}

pub fn request_id(value: &Value) -> Option<String> {
    value.get("requestId")
        .and_then(Value::as_str)
        .map(str::to_owned)
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

pub fn admin_allowed(state: &AppState, d: &Value) -> bool {
    d.get("adminKey")
        .and_then(Value::as_str)
        .unwrap_or_default()
        == state.config.api.admin_password
}

pub fn extract_client_ip(headers: &HeaderMap, fallback: SocketAddr) -> String {
    if let Some(value) = headers.get("x-forwarded-for").and_then(|value| value.to_str().ok()) {
        if let Some(ip) = value.split(',').next() {
            return ip.trim().to_owned();
        }
    }

    if let Some(value) = headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
    {
        return value.trim().to_owned();
    }

    fallback.ip().to_string()
}

pub fn random_session_id() -> String {
    let mut rng = thread_rng();
    std::iter::repeat_with(|| rng.sample(Alphanumeric))
        .map(char::from)
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(32)
        .collect()
}

pub fn random_message_id() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
