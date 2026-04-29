use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, Path as AxumPath, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tower_http::cors::{Any, CorsLayer};

use crate::{
    accounts::{user_response, username_hits_blocklist},
    state::SharedState,
    utils::extract_client_ip,
    websocket::handle_socket,
};

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(webchat_page))
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/register", post(auth_register))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/recover", post(auth_recover))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/username", post(auth_username))
        .route("/api/admin/overview", get(admin_overview))
        .route("/api/admin/features", post(admin_features))
        .route(
            "/api/admin/users/:user_id/disabled",
            post(admin_user_disabled),
        )
        .route("/ws", get(ws_upgrade))
        .route("/*path", get(public_asset))
        .layer(cors_layer())
        .with_state(state)
}

fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

#[derive(Debug, Deserialize)]
struct AuthRegisterRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct AuthLoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRecoverRequest {
    username: String,
    recovery_words: String,
    new_password: String,
}

#[derive(Debug, Deserialize)]
struct UsernameRequest {
    username: String,
}

#[derive(Debug, Deserialize)]
struct FeatureRequest {
    key: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct DisabledRequest {
    disabled: bool,
}

async fn auth_register(
    State(state): State<SharedState>,
    Json(body): Json<AuthRegisterRequest>,
) -> impl IntoResponse {
    match state.accounts.feature_flags().await {
        Ok(flags) if flags.register_enabled => {}
        Ok(_) => return api_error(StatusCode::FORBIDDEN, "Registrations are disabled."),
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
    if username_hits_blocklist(&body.username, &state.blocklist_terms) {
        return api_error(StatusCode::BAD_REQUEST, "Username is not allowed.");
    }
    match state
        .accounts
        .register(&body.username, &body.password)
        .await
    {
        Ok((user, token, recovery_words)) => Json(json!({
            "ok": true,
            "token": token,
            "user": user,
            "recoveryWords": recovery_words
        }))
        .into_response(),
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn auth_login(
    State(state): State<SharedState>,
    Json(body): Json<AuthLoginRequest>,
) -> impl IntoResponse {
    match state.accounts.login(&body.username, &body.password).await {
        Ok((user, token)) => Json(user_response(user, token)).into_response(),
        Err(err) => api_error(StatusCode::UNAUTHORIZED, &err),
    }
}

async fn auth_recover(
    State(state): State<SharedState>,
    Json(body): Json<AuthRecoverRequest>,
) -> impl IntoResponse {
    match state
        .accounts
        .recover(&body.username, &body.recovery_words, &body.new_password)
        .await
    {
        Ok((user, token)) => Json(user_response(user, token)).into_response(),
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn auth_me(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(token) = bearer_token(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "Missing session.");
    };
    match state.accounts.me(&token).await {
        Ok(Some(user)) => Json(json!({ "ok": true, "user": user })).into_response(),
        Ok(None) => api_error(StatusCode::UNAUTHORIZED, "Invalid session."),
        Err(err) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

async fn auth_logout(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(token) = bearer_token(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "Missing session.");
    };
    match state.accounts.logout(&token).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

async fn auth_username(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<UsernameRequest>,
) -> impl IntoResponse {
    if username_hits_blocklist(&body.username, &state.blocklist_terms) {
        return api_error(StatusCode::BAD_REQUEST, "Username is not allowed.");
    }
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    match state
        .accounts
        .change_username(&user.id, &body.username)
        .await
    {
        Ok(user) => Json(json!({ "ok": true, "user": user })).into_response(),
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn admin_overview(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }

    let users = match state.accounts.list_users().await {
        Ok(users) => users,
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    };
    let features = match state.accounts.feature_flags().await {
        Ok(features) => features,
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    };
    let mut room_previews: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let online_count = {
        let players = state.players.read().await;
        for player in players.values() {
            for room_id in &player.rooms {
                let entry = room_previews.entry(room_id.clone()).or_insert_with(|| {
                    json!({
                        "roomId": room_id,
                        "messageCount": 0usize,
                        "lastMessageAt": 0u64,
                        "onlineCount": 0usize,
                        "voiceCount": 0usize,
                        "active": true
                    })
                });
                entry["onlineCount"] = json!(entry["onlineCount"].as_u64().unwrap_or(0) + 1);
                if player.is_voice_chat {
                    entry["voiceCount"] = json!(entry["voiceCount"].as_u64().unwrap_or(0) + 1);
                }
            }
        }
        players.len()
    };
    {
        let rooms = state.room_messages.read().await;
        for (room_id, messages) in rooms.iter() {
            let last = messages.last();
            let entry = room_previews.entry(room_id.clone()).or_insert_with(|| {
                json!({
                    "roomId": room_id,
                    "messageCount": 0usize,
                    "lastMessageAt": 0u64,
                    "onlineCount": 0usize,
                    "voiceCount": 0usize,
                    "active": false
                })
            });
            entry["messageCount"] = json!(messages.len());
            entry["lastMessageAt"] = json!(last.map(|message| message.timestamp).unwrap_or(0));
        }
    }
    let rooms = room_previews.into_values().collect::<Vec<_>>();

    Json(json!({
        "ok": true,
        "users": users,
        "features": features,
        "rooms": rooms,
        "onlineCount": online_count
    }))
    .into_response()
}

async fn admin_features(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<FeatureRequest>,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }
    let key = match body.key.as_str() {
        "registerEnabled" | "register_enabled" => "register_enabled",
        "callsEnabled" | "calls_enabled" => "calls_enabled",
        _ => return api_error(StatusCode::BAD_REQUEST, "Unknown feature."),
    };
    match state.accounts.set_feature(key, body.enabled).await {
        Ok(()) => match state.accounts.feature_flags().await {
            Ok(features) => Json(json!({ "ok": true, "features": features })).into_response(),
            Err(err) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
        },
        Err(err) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

async fn admin_user_disabled(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<DisabledRequest>,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }
    match state
        .accounts
        .set_user_disabled(&user_id, body.disabled)
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    }
}

async fn authenticated_user(
    state: &SharedState,
    headers: &HeaderMap,
) -> Option<crate::accounts::AuthenticatedUser> {
    let token = bearer_token(headers)?;
    state
        .accounts
        .authenticate_token(&token)
        .await
        .ok()
        .flatten()
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

async fn webchat_page(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let path =
        PathBuf::from(&state.config.network.public_dir).join(&state.config.network.webchat_index);
    let origin = public_origin(&headers, &state.config.api.public_domain);
    serve_webchat_index(&path, origin.as_deref(), &state).await
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

async fn serve_webchat_index(path: &Path, origin: Option<&str>, state: &SharedState) -> Response {
    match fs::read_to_string(path).await {
        Ok(html) => {
            let html = match origin {
                Some(origin) => absolutize_social_meta(&html, origin),
                None => html,
            };
            let html = inject_runtime_config(&html, state, origin);

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

fn inject_runtime_config(html: &str, state: &SharedState, origin: Option<&str>) -> String {
    let rtc = &state.config.rtc;
    let server_origin = origin
        .map(str::to_owned)
        .or_else(|| configured_public_origin(&state.config.api.public_domain));
    let ws_url = server_origin.as_deref().and_then(websocket_url_for_origin);
    let relay_ready = !rtc.turn_urls.is_empty()
        && !rtc.turn_username.trim().is_empty()
        && !rtc.turn_credential.trim().is_empty();
    let calls_enabled = if rtc.relay_only { relay_ready } else { true };
    let calls_unavailable_reason = if calls_enabled {
        String::new()
    } else if rtc.relay_only {
        "Calls are disabled until a TURN relay is configured by the server admin.".to_owned()
    } else {
        String::new()
    };

    let mut payload = json!({
        "rtc": {
            "relayOnly": rtc.relay_only,
            "turnUrls": rtc.turn_urls,
            "turnUsername": rtc.turn_username,
            "turnCredential": rtc.turn_credential,
            "callsEnabled": calls_enabled,
            "callsUnavailableReason": calls_unavailable_reason
        }
    });
    if let Some(server_origin) = server_origin {
        payload["serverOrigin"] = json!(server_origin);
        payload["apiBaseUrl"] = json!(server_origin);
    }
    if let Some(ws_url) = ws_url {
        payload["wsUrl"] = json!(ws_url);
    }
    let script = format!(r#"<script>window.__QXP_RUNTIME__ = {};</script>"#, payload);
    html.replace("</head>", &format!("{script}\n  </head>"))
}

fn configured_public_origin(public_domain: &str) -> Option<String> {
    let value = public_domain.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        let parsed = url::Url::parse(value).ok()?;
        if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some() {
            return Some(value.to_owned());
        }
        return None;
    }

    let host = sanitized_host(value)?;
    let proto = if host.starts_with("localhost")
        || host.starts_with("127.")
        || host.starts_with("[::1]")
    {
        "http"
    } else {
        "https"
    };
    Some(format!("{proto}://{host}"))
}

fn websocket_url_for_origin(origin: &str) -> Option<String> {
    let parsed = url::Url::parse(origin).ok()?;
    let scheme = match parsed.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => return None,
    };
    let host = parsed.host_str()?;
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Some(format!("{scheme}://{authority}/ws"))
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
