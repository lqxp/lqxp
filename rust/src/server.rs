use std::{
    collections::{BTreeMap, HashSet},
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

async fn app_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let path = path.trim_start_matches('/');

    // /app/foo -> foo
    let path = path.strip_prefix("app/").unwrap_or(path);

    let full_path = PathBuf::from(&state.config.network.public_dir).join(path);

    // Si quelqu'un ouvre /app/nimportequoi
    // on renvoie le SPA
    if !full_path.exists() {
        let index = PathBuf::from(&state.config.network.public_dir)
            .join(&state.config.network.webchat_index);

        let origin = None;

        return serve_webchat_index(&index, origin, &state).await;
    }

    serve_file(&full_path).await
}

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        // Frontend React/Vite sous /app
        .route("/app", get(webchat_page))
        .route("/app/*path", get(app_asset))
        // API
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/register", post(auth_register))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/recover", post(auth_recover))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/delete", post(auth_delete))
        .route("/api/auth/username", post(auth_username))
        .route("/api/admin/overview", get(admin_overview))
        .route("/api/admin/features", post(admin_features))
        .route(
            "/api/admin/users/:user_id/disabled",
            post(admin_user_disabled),
        )
        .route("/api/admin/users/:user_id/banned", post(admin_user_banned))
        // Websocket
        .route("/ws", get(ws_upgrade))
        // Ancien serveur statique si besoin
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
struct AuthDeleteRequest {
    password: String,
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

#[derive(Debug, Deserialize)]
struct BannedRequest {
    banned: bool,
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

async fn auth_delete(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<AuthDeleteRequest>,
) -> impl IntoResponse {
    let Some(token) = bearer_token(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "Missing session.");
    };
    match state.accounts.delete_account(&token, &body.password).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
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
        Ok(updated_user) => {
            let mut touched_rooms = HashSet::new();
            {
                let mut players = state.players.write().await;
                for player in players.values_mut() {
                    if player.user_id == updated_user.id {
                        player.username = updated_user.username.clone();
                        touched_rooms.extend(player.rooms.iter().cloned());
                    }
                }
            }

            for room_id in touched_rooms {
                let payload = {
                    let players = state.players.read().await;
                    let room_players = players
                        .values()
                        .filter(|player| {
                            player.rooms.contains(&room_id)
                                && player.status != crate::models::UserPresenceStatus::Invisible
                                && !player.username.trim().is_empty()
                        })
                        .collect::<Vec<_>>();

                    let users = room_players
                        .iter()
                        .map(|player| player.username.clone())
                        .collect::<Vec<_>>();

                    let profiles = room_players
                        .iter()
                        .fold(BTreeMap::new(), |mut acc, player| {
                            acc.insert(player.username.clone(), player.profile.clone());
                            acc
                        });

                    let statuses = room_players
                        .iter()
                        .fold(BTreeMap::new(), |mut acc, player| {
                            acc.insert(player.username.clone(), player.status);
                            acc
                        });

                    let platforms = room_players
                        .iter()
                        .fold(BTreeMap::new(), |mut acc, player| {
                            acc.insert(player.username.clone(), player.platform.clone());
                            acc
                        });

                    let voice_players = room_players
                        .iter()
                        .filter(|player| player.is_voice_chat)
                        .map(|player| player.username.clone())
                        .collect::<Vec<_>>();

                    let call_players = room_players
                        .iter()
                        .filter(|player| player.is_voice_chat)
                        .map(|player| {
                            json!({
                                "username": player.username,
                                "clientId": player.client_id,
                                "platform": player.platform,
                                "isVoiceChat": player.is_voice_chat,
                                "media": {
                                    "audio": player.is_voice_chat,
                                    "camera": player.call_camera,
                                    "screen": player.call_screen
                                }
                            })
                        })
                        .collect::<Vec<_>>();

                    json!({
                        "op": 3,
                        "d": {
                            "ok": true,
                            "system": true,
                            "gameId": room_id,
                            "players": users,
                            "profiles": profiles,
                            "statuses": statuses,
                            "platforms": platforms,
                            "voicePlayers": voice_players,
                            "callPlayers": call_players
                        }
                    })
                };

                let room_txs = {
                    let players = state.players.read().await;
                    players
                        .values()
                        .filter(|player| player.rooms.contains(&room_id))
                        .map(|player| player.tx.clone())
                        .collect::<Vec<_>>()
                };

                for tx in room_txs {
                    crate::utils::send_json(&tx, payload.clone());
                }
            }

            Json(json!({ "ok": true, "user": updated_user })).into_response()
        }
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
    if user.id == user_id {
        return api_error(
            StatusCode::BAD_REQUEST,
            "You cannot disable your own account.",
        );
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

async fn admin_user_banned(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<BannedRequest>,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }
    if user.id == user_id {
        return api_error(StatusCode::BAD_REQUEST, "You cannot ban your own account.");
    }
    match state.accounts.set_user_banned(&user_id, body.banned).await {
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
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_webchat_index(path: &Path, origin: Option<&str>, state: &SharedState) -> Response {
    match fs::read_to_string(path).await {
        Ok(mut html) => {
            let runtime = runtime_config_payload(origin, state);
            let bootstrap = format!("<script>window.__QXP_RUNTIME__ = {};</script>", runtime);
            if html.contains("</head>") {
                html = html.replacen("</head>", &format!("{bootstrap}</head>"), 1);
            } else {
                html = format!("{bootstrap}{html}");
            }
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html; charset=utf-8")
                .body(axum::body::Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn runtime_config_payload(origin: Option<&str>, state: &SharedState) -> serde_json::Value {
    let public_domain = state.config.api.public_domain.trim();
    let ws_origin = origin
        .map(str::to_owned)
        .or_else(|| {
            if public_domain.is_empty() {
                None
            } else {
                Some(format!("https://{public_domain}"))
            }
        })
        .unwrap_or_default();

    let ws_url = if ws_origin.is_empty() {
        String::new()
    } else if ws_origin.starts_with("https://") {
        format!("wss://{}/ws", ws_origin.trim_start_matches("https://"))
    } else if ws_origin.starts_with("http://") {
        format!("ws://{}/ws", ws_origin.trim_start_matches("http://"))
    } else {
        format!("wss://{ws_origin}/ws")
    };

    let rtc = &state.config.rtc;
    let calls_enabled = true;
    let calls_unavailable_reason = String::new();

    let mut payload = json!({
        "rtc": {
            "relayOnly": rtc.relay_only,
            "turnUrls": rtc.turn_urls,
            "turnUsername": rtc.turn_username,
            "turnCredential": rtc.turn_credential,
            "callsEnabled": calls_enabled,
            "callsUnavailableReason": calls_unavailable_reason
        },
        "api": {
            "origin": origin.unwrap_or_default(),
            "publicDomain": public_domain,
            "wsUrl": ws_url
        }
    });

    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "latestVersion".to_owned(),
            json!(state
                .config
                .network
                .latest_version
                .as_deref()
                .unwrap_or("")
                .trim()),
        );
    }

    payload
}

fn public_origin(headers: &HeaderMap, configured_domain: &str) -> Option<String> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(host) = host {
        let forwarded_proto = headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("https");
        return Some(format!("{forwarded_proto}://{host}"));
    }

    let configured = configured_domain.trim();
    if configured.is_empty() {
        return None;
    }
    Some(format!("https://{configured}"))
}

async fn ws_upgrade(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let ip = extract_client_ip(&headers, addr);
    ws.on_upgrade(move |socket| handle_socket(state, socket, ip))
}
