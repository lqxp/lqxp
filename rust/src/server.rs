use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::fs;

use crate::{state::SharedState, utils::extract_client_ip, websocket::handle_socket};

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(webchat_page))
        .route("/ws", get(ws_upgrade))
        .route("/*path", get(public_asset))
        .with_state(state)
}

async fn webchat_page(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let path =
        PathBuf::from(&state.config.network.public_dir).join(&state.config.network.webchat_index);
    let origin = public_origin(&headers, &state.config.api.domain);
    serve_webchat_index(&path, origin.as_deref()).await
}

async fn public_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let sanitized = path.trim_start_matches('/');
    let full_path = PathBuf::from(&state.config.network.public_dir).join(sanitized);
    serve_file(&full_path).await
}

async fn serve_file(path: &Path) -> Response {
    match fs::read(path).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime.as_ref())
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!("Resource not found: {}", path.display()),
        )
            .into_response(),
    }
}

async fn serve_webchat_index(path: &Path, origin: Option<&str>) -> Response {
    match fs::read_to_string(path).await {
        Ok(html) => {
            let html = match origin {
                Some(origin) => absolutize_social_meta(&html, origin),
                None => html,
            };

            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html; charset=utf-8")
                .body(axum::body::Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!("Resource not found: {}", path.display()),
        )
            .into_response(),
    }
}

fn absolutize_social_meta(html: &str, origin: &str) -> String {
    html.replace(r#"href="/""#, &format!(r#"href="{origin}/""#))
        .replace(r#"content="/""#, &format!(r#"content="{origin}/""#))
        .replace(
            r#"content="/social-card.png""#,
            &format!(r#"content="{origin}/social-card.png""#),
        )
}

fn public_origin(headers: &HeaderMap, fallback_domain: &str) -> Option<String> {
    let host = header_first(headers, "x-forwarded-host")
        .or_else(|| header_first(headers, "host"))
        .or_else(|| sanitized_host(fallback_domain))?;
    let proto = header_first(headers, "x-forwarded-proto")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "http" | "https" => Some(value),
            _ => None,
        })
        .unwrap_or_else(|| {
            if host.starts_with("localhost")
                || host.starts_with("127.")
                || host.starts_with("[::1]")
            {
                "http".to_owned()
            } else {
                "https".to_owned()
            }
        });

    Some(format!("{proto}://{host}"))
}

fn header_first(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(sanitized_host)
}

fn sanitized_host(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 253
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b':' | b'[' | b']'))
    {
        return None;
    }

    Some(value.to_owned())
}

async fn ws_upgrade(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let ip = extract_client_ip(&headers, addr);
    ws.max_frame_size(32 * 1024 * 1024)
        .max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| handle_socket(state, socket, ip))
}
