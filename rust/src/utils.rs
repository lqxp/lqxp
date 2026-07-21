use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::extract::ws::Message;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    models::StoredFile,
    state::{AppState, RateLimitBucket},
};

pub fn send_json(tx: &mpsc::UnboundedSender<Message>, payload: Value) {
    let _ = tx.send(Message::Text(payload.to_string().into()));
}

pub fn request_id(value: &Value) -> Option<String> {
    value
        .get("requestId")
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

pub async fn rate_limit_hit(
    state: &AppState,
    key: impl Into<String>,
    limit: u32,
    window_ms: u64,
) -> bool {
    let now = now_ms();
    let key = key.into();
    let mut buckets = state.rate_limits.lock().await;
    if buckets.len() > 10_000 {
        buckets.retain(|_, bucket| now.saturating_sub(bucket.window_start_ms) <= window_ms * 2);
    }
    let bucket = buckets.entry(key).or_insert(RateLimitBucket {
        window_start_ms: now,
        count: 0,
    });
    if now.saturating_sub(bucket.window_start_ms) > window_ms {
        bucket.window_start_ms = now;
        bucket.count = 0;
    }
    bucket.count = bucket.count.saturating_add(1);
    bucket.count > limit
}

pub fn sanitize_filename(value: &str, fallback: &str, max_len: usize) -> String {
    let mut sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    sanitized = sanitized.trim_matches('.').trim().to_owned();
    if sanitized.is_empty() {
        sanitized = fallback.to_owned();
    }
    sanitized.chars().take(max_len).collect()
}

pub fn public_file_url(state: &AppState, file_id: &str) -> String {
    let base = state.config.network.upload_public_base.trim();
    let base = if base.is_empty() { "/uploads" } else { base };
    let base = if base.starts_with('/') {
        base.trim_end_matches('/').to_owned()
    } else {
        format!("/{}", base.trim_matches('/'))
    };
    format!("{}/{}", base, file_id)
}

pub async fn store_uploaded_bytes(
    state: &AppState,
    extension: &str,
    bytes: &[u8],
    mime_type: &str,
) -> Result<StoredFile, String> {
    let file_id = if extension.trim().is_empty() {
        Uuid::new_v4().simple().to_string()
    } else {
        format!(
            "{}.{}",
            Uuid::new_v4().simple(),
            extension.trim().trim_start_matches('.')
        )
    };
    let path: PathBuf = Path::new(&state.config.network.upload_dir).join(&file_id);
    std::fs::write(&path, bytes).map_err(|err| format!("Failed to store file: {err}"))?;
    Ok(StoredFile {
        id: file_id.clone(),
        url: public_file_url(state, &file_id),
        size: bytes.len() as u64,
        mime_type: mime_type.to_owned(),
    })
}
