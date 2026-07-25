use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{ws::{Message, WebSocketUpgrade}, ConnectInfo, DefaultBodyLimit, Multipart, Path as AxumPath, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    accounts::{user_response, username_hits_blocklist},
    models::{ProfileImage, RoomIcon, RoomRecord},
    state::SharedState,
    utils::{send_json, store_uploaded_bytes},
    websocket::{handle_socket, protocol},
};

async fn app_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let Some(full_path) = safe_child_path(&state.config.network.public_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if !full_path.exists() {
        let index = PathBuf::from(&state.config.network.public_dir)
            .join(&state.config.network.webchat_index);
        return serve_webchat_index(&index, None, &state).await;
    }

    serve_file(&full_path).await
}

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        // Frontend React/Vite sous /app
        .route("/app", get(webchat_page))
        .route("/app/", get(webchat_page))
        .route("/app/uploads/*path", get(upload_asset))
        .route("/app/*path", get(app_asset))
        // API
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/register", post(auth_register))
        .route("/api/auth/login", post(auth_login))
        .route("/api/auth/recover", post(auth_recover))
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/delete", post(auth_delete))
        .route("/api/auth/username", post(auth_username))
        .route("/api/profile/image", post(profile_image_upload))
        .route("/api/rooms/:room_id/icon", post(room_icon_upload))
        .route("/api/admin/overview", get(admin_overview))
        .route("/api/admin/features", post(admin_features))
        .route(
            "/api/admin/users/:user_id/disabled",
            post(admin_user_disabled),
        )
        .route("/api/admin/users/:user_id/banned", post(admin_user_banned))
        .route("/api/admin/users/:user_id/delete", post(admin_user_delete))
        .route("/api/admin/users/:user_id/badges", post(admin_user_badges))
        .route("/api/release", get(latest_release))
        // Websocket
        .route("/ws", get(ws_upgrade))
        // Ancien serveur statique si besoin
        .route("/*path", get(public_asset))
        .layer(DefaultBodyLimit::max(6 * 1024 * 1024))
        .layer(cors_layer(&state))
        .with_state(state)
}

fn cors_layer(state: &SharedState) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(allowed_cors_origins(state))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

fn allowed_cors_origins(state: &SharedState) -> AllowOrigin {
    let mut origins = [
        state.config.api.public_domain.trim(),
        state.config.api.domain.trim(),
    ]
    .into_iter()
    .filter(|origin| !origin.is_empty())
    .flat_map(|origin| {
        if origin.starts_with("http://") || origin.starts_with("https://") {
            vec![origin.to_owned()]
        } else {
            vec![format!("https://{origin}"), format!("http://{origin}")]
        }
    })
    .filter_map(|origin| origin.parse().ok())
    .collect::<Vec<_>>();

    origins.extend([
        "tauri://localhost".parse().unwrap(),
        "http://tauri.localhost".parse().unwrap(),
        "https://tauri.localhost".parse().unwrap(),
        "http://localhost:4173".parse().unwrap(),
        "http://127.0.0.1:4173".parse().unwrap(),
        "http://localhost:5173".parse().unwrap(),
        "http://127.0.0.1:5173".parse().unwrap(),
    ]);

    origins.sort();
    origins.dedup();

    AllowOrigin::list(origins)
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

#[derive(Debug, Deserialize)]
struct BadgesRequest {
    badges: Vec<String>,
}

const MAX_PROFILE_AVATAR_BYTES: usize = 2 * 1024 * 1024;
const MAX_PROFILE_BANNER_BYTES: usize = 5 * 1024 * 1024;
const MAX_ROOM_ICON_UPLOAD_BYTES: usize = 5 * 1024 * 1024;

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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<AuthLoginRequest>,
) -> impl IntoResponse {
    let client_ip = client_ip(&headers, addr);
    if crate::utils::rate_limit_hit(state.as_ref(), format!("login:ip:{client_ip}"), 10, 15 * 60_000).await {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "Too many login attempts.");
    }

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
        Ok(Some((user, token))) => Json(user_response(user, token)).into_response(),
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
        Err(err) if err.contains("once per week") => api_error(StatusCode::TOO_MANY_REQUESTS, &err),
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn profile_image_upload(
    State(state): State<SharedState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };

    let mut kind = String::new();
    let mut file_name = String::new();
    let mut declared_mime = String::new();
    let mut bytes = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "kind" {
            kind = field.text().await.unwrap_or_default();
            continue;
        }
        if name == "file" {
            file_name = field.file_name().unwrap_or("profile-image").to_owned();
            declared_mime = field.content_type().unwrap_or_default().to_owned();
            bytes = field.bytes().await.map(|value| value.to_vec()).unwrap_or_default();
        }
    }

    let kind = kind.trim();
    let max_bytes = match kind {
        "avatar" => MAX_PROFILE_AVATAR_BYTES,
        "banner" => MAX_PROFILE_BANNER_BYTES,
        _ => return api_error(StatusCode::BAD_REQUEST, "Invalid profile image kind."),
    };
    if bytes.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "Missing file.");
    }
    if bytes.len() > max_bytes {
        return api_error(StatusCode::PAYLOAD_TOO_LARGE, "Profile image too large.");
    }

    let (mime_type, width, height) = match protocol::detect_profile_image(&bytes, &declared_mime) {
        Ok(result) => result,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, message),
    };
    let extension = file_extension(&file_name, mime_type);
    let stored = match store_uploaded_bytes(state.as_ref(), extension, &bytes, mime_type).await {
        Ok(stored) => stored,
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    };
    let image = ProfileImage { file: stored, width, height };

    let (profile, sessions) = {
        let mut players = state.players.write().await;
        let mut profile = None;
        let mut sessions = Vec::new();
        for player in players.values_mut().filter(|player| player.user_id == user.id) {
            match kind {
                "avatar" => player.profile.avatar = Some(image.clone()),
                "banner" => player.profile.banner = Some(image.clone()),
                _ => {}
            }
            profile = Some(player.profile.clone());
            sessions.push((
                player.username.clone(),
                player.status,
                player.client_id.clone(),
                player.platform.clone(),
                player.rooms.iter().cloned().collect::<Vec<_>>(),
            ));
        }
        let mut profile = profile.unwrap_or(user.profile.clone());
        match kind {
            "avatar" => profile.avatar = Some(image.clone()),
            "banner" => profile.banner = Some(image.clone()),
            _ => {}
        }
        (profile, sessions)
    };

    if let Err(err) = state.accounts.update_profile(&user.id, &profile).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err);
    }

    broadcast_profile_update(&state, sessions, profile.clone()).await;

    Json(json!({ "ok": true, "profile": profile, kind: image })).into_response()
}

async fn room_icon_upload(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(room_id): AxumPath<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    let room_id = room_id.trim().to_owned();
    if room_id.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "Invalid room.");
    }

    let is_member = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| player.user_id == user.id)
            .any(|player| player.rooms.contains(&room_id))
    };
    if !is_member {
        return api_error(StatusCode::FORBIDDEN, "You are not in this room.");
    }

    let mut file_name = String::new();
    let mut declared_mime = String::new();
    let mut bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name().unwrap_or_default() == "file" {
            file_name = field.file_name().unwrap_or("room-icon").to_owned();
            declared_mime = field.content_type().unwrap_or_default().to_owned();
            bytes = field.bytes().await.map(|value| value.to_vec()).unwrap_or_default();
        }
    }
    if bytes.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "Missing file.");
    }
    if bytes.len() > MAX_ROOM_ICON_UPLOAD_BYTES {
        return api_error(StatusCode::PAYLOAD_TOO_LARGE, "Room icon too large.");
    }

    let (mime_type, _, _) = match protocol::detect_profile_image(&bytes, &declared_mime) {
        Ok(result) => result,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, message),
    };
    let extension = file_extension(&file_name, mime_type);

    if let Some(previous_icon) = state.database.room_icon(&room_id).await {
        let previous_path = Path::new(&state.config.network.upload_dir).join(&previous_icon.file.id);
        let _ = fs::remove_file(previous_path).await;
    }

    let stored = match store_uploaded_bytes(state.as_ref(), extension, &bytes, mime_type).await {
        Ok(stored) => stored,
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    };
    let icon = RoomIcon { file: stored };
    if let Err(err) = state.database.set_room_icon(&room_id, &icon).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string());
    }
    if let Err(err) = protocol::sync_room_record(&state, &room_id).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string());
    }

    let room = state.database.room_record(&room_id).await.unwrap_or(RoomRecord {
        room_id: room_id.clone(),
        title: room_id.clone(),
        icon: Some(icon.clone()),
        members: protocol::room_usernames(&state, &room_id, None).await,
    });

    protocol::broadcast_to_room(
        &state,
        &room_id,
        json!({
            "op": 32,
            "d": {
                "ok": true,
                "system": true,
                "gameId": room_id,
                "room": room
            }
        }),
    )
    .await;

    Json(json!({ "ok": true, "icon": icon, "room": room })).into_response()
}

async fn broadcast_profile_update(
    state: &SharedState,
    sessions: Vec<(String, crate::models::UserPresenceStatus, String, String, Vec<String>)>,
    profile: crate::models::UserProfile,
) {
    for (username, status, client_id, platform, rooms) in sessions {
        if status == crate::models::UserPresenceStatus::Invisible {
            continue;
        }
        for room in rooms {
            let roster = protocol::room_players(state, &room, None).await;
            let profiles = protocol::room_profiles(state, &room, None).await;
            let statuses = protocol::room_statuses(state, &room, None).await;
            let platforms = protocol::room_platforms(state, &room, None).await;
            let voice_roster = protocol::room_voice_usernames(state, &room, None).await;
            let call_players = protocol::room_call_players(state, &room, None).await;
            protocol::broadcast_to_room(
                state,
                &room,
                json!({
                    "op": 26,
                    "d": {
                        "gameId": room,
                        "user": username,
                        "profile": profile,
                        "players": roster,
                        "profiles": profiles,
                        "statuses": statuses,
                        "platforms": platforms,
                        "voicePlayers": voice_roster,
                        "callPlayers": call_players,
                        "clientId": client_id,
                        "platform": platform
                    }
                }),
            )
            .await;
        }
    }
}

fn file_extension<'a>(file_name: &'a str, mime_type: &str) -> &'a str {
    file_name
        .rsplit('.')
        .next()
        .filter(|segment| *segment != file_name)
        .unwrap_or(match mime_type {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/jpeg" => "jpg",
            _ => "bin",
        })
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

async fn admin_user_delete(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }
    if user.id == user_id {
        return api_error(StatusCode::BAD_REQUEST, "You cannot delete your own account.");
    }
    match state.accounts.delete_user_account(&user_id).await {
        Ok(()) => {
            disconnect_user_sessions(&state, &user_id).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn admin_user_badges(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<BadgesRequest>,
) -> impl IntoResponse {
    let Some(user) = authenticated_user(&state, &headers).await else {
        return api_error(StatusCode::UNAUTHORIZED, "Invalid session.");
    };
    if !user.admin {
        return api_error(StatusCode::FORBIDDEN, "Admin only.");
    }
    match state.accounts.set_user_badges(&user_id, &body.badges).await {
        Ok(updated_user) => {
            broadcast_badge_update(&state, &updated_user).await;
            Json(json!({ "ok": true, "user": updated_user })).into_response()
        }
        Err(err) => api_error(StatusCode::BAD_REQUEST, &err),
    }
}

async fn broadcast_badge_update(state: &SharedState, user: &crate::accounts::PublicUser) {
    let mut touched_rooms = HashSet::new();
    let mut txs = Vec::new();
    {
        let mut players = state.players.write().await;
        for player in players.values_mut().filter(|player| player.user_id == user.id) {
            player.badges = user.badges.clone();
            touched_rooms.extend(player.rooms.iter().cloned());
            txs.push(player.tx.clone());
        }
    }

    for tx in txs {
        send_json(
            &tx,
            json!({
                "op": 34,
                "d": {
                    "ok": true,
                    "user": user.username,
                    "username": user.username,
                    "userId": user.id,
                    "badges": user.badges
                }
            }),
        );
    }

    for room_id in touched_rooms {
        let players = protocol::room_players(state, &room_id, None).await;
        let room_txs = {
            let players = state.players.read().await;
            players
                .values()
                .filter(|player| player.rooms.contains(&room_id))
                .map(|player| player.tx.clone())
                .collect::<Vec<_>>()
        };
        for tx in room_txs {
            send_json(
                &tx,
                json!({
                    "op": 34,
                    "d": {
                        "ok": true,
                        "gameId": room_id,
                        "user": user.username,
                        "username": user.username,
                        "userId": user.id,
                        "badges": user.badges,
                        "players": players
                    }
                }),
            );
        }
    }
}

async fn disconnect_user_sessions(state: &SharedState, user_id: &str) {
    let sessions = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| player.user_id == user_id)
            .map(|player| (player.id.clone(), player.tx.clone()))
            .collect::<Vec<_>>()
    };

    for (session_id, tx) in sessions {
        send_json(&tx, json!({ "op": 0, "d": { "error": "Account deleted." } }));
        let _ = tx.send(Message::Close(None));
        crate::websocket::disconnect_player(state, &session_id).await;
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

fn client_ip(_headers: &HeaderMap, addr: SocketAddr) -> String {
    addr.ip().to_string()
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

async fn latest_release() -> Response {
    let client = reqwest::Client::builder()
        .user_agent("QxProtocol-ReleaseProxy/0.1 (+https://github.com/lqxp)")
        .build()
        .expect("failed to build reqwest client");

    let response = match client
        .get("https://api.github.com/repos/lqxp/app/releases/latest")
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "Failed to fetch release."),
    };

    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(_) => return api_error(StatusCode::BAD_GATEWAY, "Failed to read release response."),
    };

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
    let Some(full_path) = safe_child_path(&state.config.network.public_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve_file(&full_path).await
}

async fn upload_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let Some(full_path) = safe_child_path(&state.config.network.upload_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve_file(&full_path).await
}

fn safe_child_path(base: &str, raw_path: &str) -> Option<PathBuf> {
    let relative = Path::new(raw_path.trim_start_matches('/'));
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(Path::new(base).join(relative))
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
    let client_ip = client_ip(&headers, addr);
    if crate::utils::rate_limit_hit(state.as_ref(), format!("ws-connect:ip:{client_ip}"), 30, 60_000).await {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}
