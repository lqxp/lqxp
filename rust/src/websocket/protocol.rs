use std::collections::HashSet;

use axum::extract::ws::Message;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::error;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

use crate::{
    models::{Attachment, BlacklistEntry, ChatMessageRecord, LoggedIpEntry, MessageReaction, PlayerStatus, SocketPayload},
    state::SharedState,
    utils::{admin_allowed, now_ms, random_message_id, request_id, send_json, with_request_id},
};

const MAX_ROOM_MESSAGES: usize = 150;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_ATTACHMENT_B64_LEN: usize = ((MAX_ATTACHMENT_BYTES + 2) / 3) * 4 + 4;
const MAX_FILENAME_LEN: usize = 128;
const MAX_MIMETYPE_LEN: usize = 96;
// Voice call chunks are ~800ms of Opus at 64–128kbps → 6–13KB raw.
// Cap generously at 512KB raw (~680KB base64) so a malicious peer can't push
// large payloads per frame.
const MAX_VOICE_CHUNK_BYTES: usize = 512 * 1024;
const MAX_VOICE_CHUNK_B64_LEN: usize = ((MAX_VOICE_CHUNK_BYTES + 2) / 3) * 4 + 4;
// Minimum spacing between voice-chunk broadcasts per session. Natural cadence
// on the client is ~800ms per chunk, so 100ms floor allows bursts without
// letting a spammer saturate the room.
const MIN_VOICE_CHUNK_INTERVAL_MS: u64 = 100;

pub async fn process_message(
    state: SharedState,
    session_id: String,
    client_ip: String,
    tx: mpsc::UnboundedSender<Message>,
    raw: String,
) -> bool {
    let payload = match serde_json::from_str::<SocketPayload>(&raw) {
        Ok(payload) => payload,
        Err(_) => return false,
    };

    match payload.op {
        0 => {
            send_json(&tx, json!({ "op": 0, "d": payload.d }));
            false
        }
        1 => {
            let players = {
                let players = state.players.read().await;
                players
                    .values()
                    .filter(|player| !player.username.trim().is_empty())
                    .map(|player| player.username.clone())
                    .collect::<Vec<_>>()
            };

            send_json(
                &tx,
                with_request_id(
                    json!({
                        "op": 1,
                        "d": {
                            "ok": true,
                            "count": players.len(),
                            "players": players
                        }
                    }),
                    request_id(&payload.d),
                ),
            );
            false
        }
        2 => identify_player(&state, &session_id, &client_ip, payload.d).await,
        3 => join_game(&state, &session_id, payload.d).await,
        4 => leave_game(&state, &session_id, payload.d).await,
        5 => report_kill(&state, &session_id, payload.d).await,
        6 => {
            send_json(
                &tx,
                with_request_id(
                    json!({
                        "op": 6,
                        "d": {
                            "v": state.config.network.latest_version
                        }
                    }),
                    request_id(&payload.d),
                ),
            );
            false
        }
        7 => send_chat_message(&state, &session_id, payload.d).await,
        15 => broadcast_alive(&state, &session_id, payload.d).await,
        16 => broadcast_exchange_end(&state, &session_id, payload.d).await,
        17 => exchange_joined(&state, &session_id, payload.d).await,
        18 => send_room_history(&state, &session_id, payload.d).await,
        19 => toggle_message_reaction(&state, &session_id, payload.d).await,
        21 => delete_message(&state, &session_id, payload.d).await,
        98 => update_voice_chat(&state, &session_id, payload.d).await,
        99 => relay_voice_data(&state, &session_id, payload.d, payload.u).await,
        100 => update_mute_state(&state, &session_id, payload.d).await,
        101 => admin_status(&state, &session_id, payload.d).await,
        102 => admin_blacklist(&state, &session_id, payload.d).await,
        103 => admin_unblacklist(&state, &session_id, payload.d).await,
        104 => admin_broadcast(&state, &session_id, payload.d).await,
        105 => stats_query(&state, &session_id, payload.d).await,
        _ => {
            send_json(
                &tx,
                json!({
                    "op": 0,
                    "d": {
                        "error": "Unknown operation."
                    }
                }),
            );
            false
        }
    }
}

async fn identify_player(state: &SharedState, session_id: &str, client_ip: &str, d: Value) -> bool {
    let mut username = d
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "Player".to_owned());

    username = username.chars().take(16).collect::<String>();

    let lowered = username.to_lowercase();
    if state
        .blocklist_terms
        .iter()
        .any(|term| lowered.contains(&term.to_lowercase()))
    {
        let entry = BlacklistEntry {
            ip: client_ip.to_owned(),
            reason: "Blocked username".to_owned(),
            timestamp: now_ms(),
            ign: username.clone(),
        };

        let mut blacklisted = state.database.blacklisted_ips().await;
        if !blacklisted.iter().any(|item| item.ip == entry.ip) {
            blacklisted.push(entry.clone());
            if let Err(err) = state.database.set_blacklisted_ips(&blacklisted).await {
                error!("Failed to update blacklist: {}", err);
            }
        }

        respond_to_sender(
            state,
            session_id,
            json!({
                "op": 24,
                "d": {
                    "error": "You are blacklisted.",
                    "reason": entry.reason,
                    "timestamp": entry.timestamp,
                    "ign": entry.ign
                }
            }),
        )
        .await;

        return true;
    }

    if username == "kxs.rip"
        && ![
            "82.67.125.203",
            "2a01:e0a:e8a:c6c0:83b:6634:e492:7cef",
            "179.61.190.52",
        ]
        .contains(&client_ip)
    {
        username = "Player".to_owned();
    }

    let existing_names = {
        let players = state.players.read().await;
        players
            .iter()
            .filter(|(id, _)| id.as_str() != session_id)
            .map(|(_, player)| player.username.to_lowercase())
            .collect::<HashSet<_>>()
    };

    let (final_username, exchange_key, voice_chat, version, is_mobile, is_secure) = {
        let mut players = state.players.write().await;
        let Some(player) = players.get_mut(session_id) else {
            return false;
        };

        let base_username = username.clone();
        let mut candidate = username;
        let mut counter = 1;
        while existing_names.contains(&candidate.to_lowercase()) {
            candidate = format!("{}-{}", base_username, counter);
            candidate = candidate.chars().take(16).collect();
            counter += 1;
        }

        player.username = candidate.clone();
        player.is_voice_chat = d.get("isVoiceChat").and_then(Value::as_bool).unwrap_or(false);
        player.version = d
            .get("v")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        player.exchange_key = d
            .get("exchangeKey")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .filter(|value| !value.trim().is_empty());
        player.is_mobile = d.get("isMobile").and_then(Value::as_bool);
        player.is_secure = d.get("isSecure").and_then(Value::as_bool);

        (
            candidate,
            player.exchange_key.clone(),
            player.is_voice_chat,
            player.version.clone(),
            player.is_mobile,
            player.is_secure,
        )
    };

    if let Err(err) = state
        .database
        .unique_push(
            "logged_ips",
            json!(LoggedIpEntry {
                ip: client_ip.to_owned(),
                username: final_username.clone(),
                version: version.clone(),
                is_voice_chat: voice_chat,
            }),
        )
        .await
    {
        error!("Failed to append logged IP: {}", err);
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 2,
                "d": {
                    "uuid": session_id
                }
            }),
            request_id(&d),
        ),
    )
    .await;

    if let Some(exchange_key) = exchange_key {
        broadcast_to_exchange_key_excluding(
            state,
            &exchange_key,
            session_id,
            json!({
                "op": 13,
                "d": {
                    "username": final_username,
                    "v": version,
                    "isSecure": is_secure,
                    "isMobile": is_mobile,
                    "isVoiceChat": voice_chat
                }
            }),
        )
        .await;
    }

    false
}

async fn join_game(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(game_id) = d.get("gameId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 3, "Malformed request", request_id(&d)).await;
    };

    if game_id.is_empty() {
        return respond_error(state, session_id, 3, "Malformed request", request_id(&d)).await;
    }

    let join_result = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            if player.username.is_empty() {
                Err("You need to be identified before")
            } else {
                let already_in = !player.rooms.insert(game_id.to_owned());
                Ok(already_in)
            }
        } else {
            Err("You need to be identified before")
        }
    };

    let already_in = match join_result {
        Ok(value) => value,
        Err(message) => return respond_error(state, session_id, 3, message, request_id(&d)).await,
    };

    let roster = room_usernames(state, game_id).await;
    let voice_roster = room_voice_usernames(state, game_id).await;

    if !already_in {
        broadcast_to_room(
            state,
            game_id,
            json!({
                "op": 3,
                "d": {
                    "ok": true,
                    "system": true,
                    "gameId": game_id,
                    "players": roster.clone(),
                    "voicePlayers": voice_roster.clone()
                }
            }),
        )
        .await;
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 3,
                "d": {
                    "ok": true,
                    "gameId": game_id,
                    "players": roster,
                    "voicePlayers": voice_roster,
                    "alreadyJoined": already_in
                }
            }),
            request_id(&d),
        ),
    )
    .await;

    dispatch_room_history(state, session_id, game_id, None).await;
    false
}

async fn leave_game(state: &SharedState, session_id: &str, d: Value) -> bool {
    let req_id = request_id(&d);

    let Some(game_id) = d.get("gameId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 4, "Missing gameId", req_id).await;
    };

    if game_id.is_empty() {
        return respond_error(state, session_id, 4, "Missing gameId", req_id).await;
    }

    let leave_result = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            if player.rooms.remove(game_id) {
                Ok(player.username.clone())
            } else {
                Err("Not a member of this room")
            }
        } else {
            Err("You need to be identified before")
        }
    };

    let username = match leave_result {
        Ok(name) => name,
        Err(message) => return respond_error(state, session_id, 4, message, req_id).await,
    };

    broadcast_to_room(
        state,
        game_id,
        json!({
            "op": 4,
            "d": {
                "gameId": game_id,
                "left": username
            }
        }),
    )
    .await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({ "op": 4, "d": { "ok": true, "gameId": game_id } }),
            req_id,
        ),
    )
    .await;

    false
}

async fn report_kill(state: &SharedState, session_id: &str, d: Value) -> bool {
    let killer = d.get("killer").and_then(Value::as_str);
    let killed = d.get("killed").and_then(Value::as_str);
    if killer.is_none() || killed.is_none() {
        return respond_error(state, session_id, 5, "Malformed request", request_id(&d)).await;
    }

    let Some(game_id) = d.get("gameId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 5, "Missing gameId", request_id(&d)).await;
    };

    if game_id.is_empty() {
        return respond_error(state, session_id, 5, "Missing gameId", request_id(&d)).await;
    }

    let is_member = {
        let players = state.players.read().await;
        players
            .get(session_id)
            .map(|p| p.rooms.contains(game_id))
            .unwrap_or(false)
    };

    if !is_member {
        return respond_error(state, session_id, 5, "Not a member of this room", request_id(&d)).await;
    }

    broadcast_to_room(
        state,
        game_id,
        json!({
            "op": 5,
            "d": {
                "gameId": game_id,
                "killer": killer.unwrap(),
                "killed": killed.unwrap(),
                "timestamp": now_ms()
            }
        }),
    )
    .await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 5, "d": { "ok": true, "gameId": game_id } }), request_id(&d)),
    )
    .await;
    false
}

async fn send_chat_message(state: &SharedState, session_id: &str, d: Value) -> bool {
    let text = d.get("text").and_then(Value::as_str).unwrap_or("");

    let Some(target_game_id) = d.get("gameId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 7, "Missing gameId", request_id(&d)).await;
    };

    if target_game_id.is_empty() {
        return respond_error(state, session_id, 7, "Missing gameId", request_id(&d)).await;
    }

    let attachment = match parse_attachment(d.get("attachment")) {
        Ok(value) => value,
        Err(message) => return respond_error(state, session_id, 7, message, request_id(&d)).await,
    };

    let trimmed = text.trim().chars().take(200).collect::<String>();
    if trimmed.is_empty() && attachment.is_none() {
        return respond_error(state, session_id, 7, "Empty message", request_id(&d)).await;
    }

    let now = now_ms();

    let chat_result = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            if player.username.is_empty() {
                Err("You need to be identified before sending message".to_owned())
            } else if !player.rooms.contains(target_game_id) {
                Err("Not a member of this room".to_owned())
            } else if let Some(last) = player.last_message_timestamp {
                if now.saturating_sub(last) < 1_000 {
                    Err(format!(
                        "You need to wait {}ms until the next message",
                        now.saturating_sub(last)
                    ))
                } else {
                    player.last_message_timestamp = Some(now);
                    Ok(player.username.clone())
                }
            } else {
                player.last_message_timestamp = Some(now);
                Ok(player.username.clone())
            }
        } else {
            Err("You need to be identified before sending message".to_owned())
        }
    };

    let player_name = match chat_result {
        Ok(name) => name,
        Err(message) => return respond_error(state, session_id, 7, &message, request_id(&d)).await,
    };

    let room_name = target_game_id.to_owned();
    let prefix = if room_name == "lobby" { "[lobby]" } else { "[in-game]" };

    let preview_target = crate::linkpreview::find_first_url(&trimmed);

    let message_record = ChatMessageRecord {
        message_id: random_message_id(),
        room_id: room_name.clone(),
        user: format!("{} {}", prefix, player_name),
        username: player_name,
        text: trimmed,
        timestamp: now,
        system: false,
        reactions: Vec::new(),
        attachment,
        preview: None,
        deleted: false,
    };

    let stored_message = store_room_message(state, &room_name, message_record).await;

    broadcast_to_room(
        state,
        &room_name,
        json!({
            "op": 7,
            "d": stored_message
        }),
    )
    .await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 7,
                "d": {
                    "ok": true,
                    "messageId": stored_message.message_id
                }
            }),
            request_id(&d),
        ),
    )
    .await;

    if let Some(url) = preview_target {
        let state_arc = state.clone();
        let room = room_name.clone();
        let msg_id = stored_message.message_id.clone();
        tokio::spawn(async move {
            let preview = crate::linkpreview::fetch_preview(&url).await;
            if let Some(preview) = preview {
                // Patch the stored record so late joiners see the preview.
                {
                    let mut rooms = state_arc.room_messages.write().await;
                    if let Some(messages) = rooms.get_mut(&room) {
                        if let Some(message) = messages.iter_mut().find(|m| m.message_id == msg_id) {
                            message.preview = Some(preview.clone());
                        }
                    }
                }
                broadcast_to_room(
                    &state_arc,
                    &room,
                    json!({
                        "op": 23,
                        "d": {
                            "gameId": room,
                            "messageId": msg_id,
                            "preview": preview
                        }
                    }),
                )
                .await;
            }
        });
    }

    false
}

async fn send_room_history(state: &SharedState, session_id: &str, d: Value) -> bool {
    let requested_room = d.get("gameId").and_then(Value::as_str);
    let room_id = match resolve_room_for_session(state, session_id, requested_room).await {
        Ok(room_id) => room_id,
        Err(message) => return respond_error(state, session_id, 18, &message, request_id(&d)).await,
    };

    dispatch_room_history(state, session_id, &room_id, request_id(&d)).await;
    false
}

async fn toggle_message_reaction(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(message_id) = d.get("messageId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 19, "Missing messageId", request_id(&d)).await;
    };
    let Some(emoji) = d.get("reaction").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 19, "Missing reaction", request_id(&d)).await;
    };

    if emoji.is_empty() {
        return respond_error(state, session_id, 19, "Missing reaction", request_id(&d)).await;
    }

    let username = {
        let players = state.players.read().await;
        if let Some(player) = players.get(session_id) {
            if player.username.is_empty() {
                return respond_error(state, session_id, 19, "You need to be identified before", request_id(&d)).await;
            }
            player.username.clone()
        } else {
            return respond_error(state, session_id, 19, "You need to be identified before", request_id(&d)).await;
        }
    };

    let room_hint = d.get("gameId").and_then(Value::as_str);
    let (room_id, reactions) = match update_message_reactions(state, message_id, emoji, &username, room_hint).await {
        Some(result) => result,
        None => return respond_error(state, session_id, 19, "Unknown messageId", request_id(&d)).await,
    };

    broadcast_to_room(
        state,
        &room_id,
        json!({
            "op": 20,
            "d": {
                "roomId": room_id,
                "messageId": message_id,
                "reactions": reactions
            }
        }),
    )
    .await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 19, "d": { "ok": true, "messageId": message_id } }), request_id(&d)),
    )
    .await;
    false
}

async fn delete_message(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(message_id) = d.get("messageId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 21, "Missing messageId", request_id(&d)).await;
    };
    if message_id.is_empty() {
        return respond_error(state, session_id, 21, "Missing messageId", request_id(&d)).await;
    }

    let room_hint = d.get("gameId").and_then(Value::as_str).map(str::trim);

    let (username, is_admin) = {
        let players = state.players.read().await;
        match players.get(session_id) {
            Some(player) if !player.username.is_empty() => {
                let is_admin = !state.config.api.admin_password.is_empty()
                    && d.get("adminKey")
                        .and_then(Value::as_str)
                        .map(|key| key == state.config.api.admin_password)
                        .unwrap_or(false);
                (player.username.clone(), is_admin)
            }
            _ => {
                return respond_error(state, session_id, 21, "You need to be identified before", request_id(&d)).await;
            }
        }
    };

    let timestamp = now_ms();
    let result = {
        let mut rooms = state.room_messages.write().await;

        let room_iter: Vec<String> = if let Some(hint) = room_hint.filter(|room| !room.is_empty()) {
            vec![hint.to_owned()]
        } else {
            rooms.keys().cloned().collect()
        };

        let mut hit: Option<(String, bool)> = None;
        for room_id in room_iter {
            if let Some(messages) = rooms.get_mut(&room_id) {
                if let Some(message) = messages.iter_mut().find(|m| m.message_id == message_id) {
                    if message.deleted {
                        hit = Some((room_id, true));
                        break;
                    }
                    if !is_admin && message.username != username {
                        return respond_error(state, session_id, 21, "Only the author can delete this message", request_id(&d)).await;
                    }
                    message.text.clear();
                    message.attachment = None;
                    message.preview = None;
                    message.reactions.clear();
                    message.deleted = true;
                    hit = Some((room_id, false));
                    break;
                }
            }
        }
        hit
    };

    let (room_id, already_deleted) = match result {
        Some(v) => v,
        None => return respond_error(state, session_id, 21, "Unknown messageId", request_id(&d)).await,
    };

    if !already_deleted {
        broadcast_to_room(
            state,
            &room_id,
            json!({
                "op": 22,
                "d": {
                    "gameId": room_id,
                    "messageId": message_id,
                    "deletedBy": username,
                    "deletedAt": timestamp
                }
            }),
        )
        .await;
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 21,
                "d": {
                    "ok": true,
                    "gameId": room_id,
                    "messageId": message_id
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn broadcast_alive(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(alive) = d.get("alive").cloned() else {
        return respond_error(state, session_id, 15, "Malformed request", request_id(&d)).await;
    };

    let exchange_key = {
        let players = state.players.read().await;
        if let Some(player) = players.get(session_id) {
            if player.rooms.is_empty() {
                Err("You need to be ingame to do that")
            } else {
                Ok(player.exchange_key.clone())
            }
        } else {
            Err("You need to be identified before")
        }
    };

    let exchange_key = match exchange_key {
        Ok(exchange_key) => exchange_key,
        Err(message) => return respond_error(state, session_id, 15, message, request_id(&d)).await,
    };

    if let Some(exchange_key) = exchange_key {
        broadcast_to_exchange_key(
            state,
            &exchange_key,
            json!({
                "op": 15,
                "d": {
                    "alive": alive
                }
            }),
        )
        .await;
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 15, "d": { "ok": true } }), request_id(&d)),
    )
    .await;
    false
}

async fn broadcast_exchange_end(state: &SharedState, session_id: &str, d: Value) -> bool {
    let payload = d.get("data").cloned().unwrap_or_else(|| json!({}));
    let exchange_key = {
        let players = state.players.read().await;
        if let Some(player) = players.get(session_id) {
            Ok(player.exchange_key.clone())
        } else {
            Err("You need to be identified before")
        }
    };

    let exchange_key = match exchange_key {
        Ok(exchange_key) => exchange_key,
        Err(message) => return respond_error(state, session_id, 16, message, request_id(&d)).await,
    };

    if let Some(exchange_key) = exchange_key {
        broadcast_to_exchange_key(
            state,
            &exchange_key,
            json!({
                "op": 16,
                "d": {
                    "data": payload
                }
            }),
        )
        .await;
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 16, "d": { "ok": true } }), request_id(&d)),
    )
    .await;
    false
}

async fn exchange_joined(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(game_id) = d.get("gameId").and_then(Value::as_str) else {
        return respond_error(state, session_id, 17, "Missing gameId", request_id(&d)).await;
    };
    let Some(exchange_key) = d.get("exchangeKey").and_then(Value::as_str) else {
        return respond_error(state, session_id, 17, "Missing exchangeKey", request_id(&d)).await;
    };

    broadcast_to_exchange_key(
        state,
        exchange_key,
        json!({
            "op": 12,
            "d": {
                "gameId": game_id,
                "exchangeKey": exchange_key
            }
        }),
    )
    .await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 17, "d": { "ok": true } }), request_id(&d)),
    )
    .await;
    false
}

async fn update_voice_chat(state: &SharedState, session_id: &str, d: Value) -> bool {
    let is_voice_chat = d.get("isVoiceChat").and_then(Value::as_bool).unwrap_or(false);
    let voice_result = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            player.is_voice_chat = is_voice_chat;
            Ok((player.username.clone(), player.rooms.iter().cloned().collect::<Vec<_>>()))
        } else {
            Err("You need to be identified before")
        }
    };

    let (username, rooms) = match voice_result {
        Ok(values) => values,
        Err(message) => return respond_error(state, session_id, 98, message, request_id(&d)).await,
    };

    for game_id in &rooms {
        broadcast_to_room(
            state,
            game_id,
            json!({
                "op": 98,
                "d": {
                    "gameId": game_id,
                    "user": username,
                    "isVoiceChat": is_voice_chat
                }
            }),
        )
        .await;
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(json!({ "op": 98, "d": { "ok": true } }), request_id(&d)),
    )
    .await;
    false
}

async fn relay_voice_data(
    state: &SharedState,
    session_id: &str,
    d: Value,
    _u: Option<String>,
) -> bool {
    let Some(game_id) = d.get("gameId").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 99, "Missing gameId", request_id(&d)).await;
    };

    if game_id.is_empty() {
        return respond_error(state, session_id, 99, "Missing gameId", request_id(&d)).await;
    }

    // Size-bound the chunk payload before doing anything else.
    let chunk = d
        .get("chunk")
        .and_then(Value::as_str)
        .unwrap_or("");
    if chunk.len() > MAX_VOICE_CHUNK_B64_LEN {
        return respond_error(state, session_id, 99, "Voice chunk too large", request_id(&d)).await;
    }

    let now = now_ms();

    // Membership + voice-enabled check + per-session rate limit, all in one write.
    let voice_context = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            if !player.rooms.contains(game_id) {
                Err("Not a member of this room")
            } else if let Some(last) = player.last_voice_chunk_timestamp {
                if now.saturating_sub(last) < MIN_VOICE_CHUNK_INTERVAL_MS {
                    Err("Voice chunk rate limited")
                } else {
                    player.last_voice_chunk_timestamp = Some(now);
                    Ok(player.username.clone())
                }
            } else {
                player.last_voice_chunk_timestamp = Some(now);
                Ok(player.username.clone())
            }
        } else {
            Err("You need to be identified before")
        }
    };

    let username = match voice_context {
        Ok(name) => name,
        Err(message) => return respond_error(state, session_id, 99, message, request_id(&d)).await,
    };

    // Build a fresh payload from validated fields only — never re-emit the raw
    // `d` blob (prevents a peer from slipping extra keys into the broadcast).
    let mime_type = d
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap_or("audio/webm")
        .chars()
        .take(MAX_MIMETYPE_LEN)
        .collect::<String>();

    let targets = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| {
                player.id != session_id
                    && player.rooms.contains(game_id)
                    && player.is_voice_chat
                    && !player.muted_users.contains(&username)
            })
            .map(|player| player.tx.clone())
            .collect::<Vec<_>>()
    };

    if targets.is_empty() {
        return false;
    }

    let payload = json!({
        "op": 99,
        "d": {
            "gameId": game_id,
            "chunk": chunk,
            "mimeType": mime_type
        },
        "u": username
    });
    let encoded = payload.to_string();
    for target in targets {
        let _ = target.send(Message::Text(encoded.clone().into()));
    }

    false
}

async fn update_mute_state(state: &SharedState, session_id: &str, d: Value) -> bool {
    let Some(username) = d.get("user").and_then(Value::as_str) else {
        return respond_error(state, session_id, 100, "Malformed request", request_id(&d)).await;
    };
    let Some(is_muted) = d.get("isMuted").and_then(Value::as_bool) else {
        return respond_error(state, session_id, 100, "Malformed request", request_id(&d)).await;
    };

    let did_update = {
        let mut players = state.players.write().await;
        if let Some(player) = players.get_mut(session_id) {
            if is_muted {
                player.muted_users.insert(username.to_owned());
            } else {
                player.muted_users.remove(username);
            }
            true
        } else {
            false
        }
    };

    if !did_update {
        return respond_error(state, session_id, 100, "You need to be identified before", request_id(&d)).await;
    }

    false
}

async fn admin_status(state: &SharedState, session_id: &str, d: Value) -> bool {
    if !admin_allowed(state, &d) {
        return respond_error(state, session_id, 101, "Unauthorized", request_id(&d)).await;
    }

    let players = {
        let players = state.players.read().await;
        players
            .values()
            .map(|player| {
                let mut rooms: Vec<String> = player.rooms.iter().cloned().collect();
                rooms.sort();
                PlayerStatus {
                    username: player.username.clone(),
                    ip: player.ip.clone(),
                    id: player.id.clone(),
                    is_voice_chat: player.is_voice_chat,
                    rooms,
                    version: player.version.clone(),
                    mobile: player.is_mobile,
                    secure_context: player.is_secure,
                }
            })
            .collect::<Vec<_>>()
    };

    let blacklisted = state.database.blacklisted_ips().await;
    let logged_ips = state.database.logged_ips().await;

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 101,
                "d": {
                    "ok": true,
                    "onlineCount": players.len(),
                    "blacklisted": blacklisted,
                    "players": players,
                    "ips": logged_ips
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn admin_blacklist(state: &SharedState, session_id: &str, d: Value) -> bool {
    if !admin_allowed(state, &d) {
        return respond_error(state, session_id, 102, "Unauthorized", request_id(&d)).await;
    }

    let Some(ip) = d.get("ip").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 102, "Missing ip", request_id(&d)).await;
    };

    let mut blacklisted = state.database.blacklisted_ips().await;
    if blacklisted.iter().any(|entry| entry.ip == ip) {
        return respond_error(state, session_id, 102, "Ip is already blacklisted", request_id(&d)).await;
    }

    let connected_player = {
        let players = state.players.read().await;
        players.values().find(|player| player.ip == ip).cloned()
    };

    let entry = BlacklistEntry {
        ip: ip.to_owned(),
        reason: d
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("No reason provided")
            .to_owned(),
        timestamp: now_ms(),
        ign: connected_player
            .as_ref()
            .map(|player| player.username.clone())
            .unwrap_or_else(|| "Unknown".to_owned()),
    };

    blacklisted.push(entry.clone());
    if let Err(err) = state.database.set_blacklisted_ips(&blacklisted).await {
        error!("Failed to persist blacklist: {}", err);
    }

    if let Some(player) = connected_player {
        send_json(
            &player.tx,
            json!({
                "op": 24,
                "d": {
                    "error": "You are blacklisted.",
                    "reason": entry.reason.clone(),
                    "timestamp": entry.timestamp,
                    "ign": entry.ign.clone()
                }
            }),
        );
        let _ = player.tx.send(Message::Close(None));
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 102,
                "d": {
                    "ok": true,
                    "ip": entry.ip,
                    "reason": entry.reason,
                    "timestamp": entry.timestamp,
                    "ign": entry.ign
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn admin_unblacklist(state: &SharedState, session_id: &str, d: Value) -> bool {
    if !admin_allowed(state, &d) {
        return respond_error(state, session_id, 103, "Unauthorized", request_id(&d)).await;
    }

    let Some(ip) = d.get("ip").and_then(Value::as_str).map(str::trim) else {
        return respond_error(state, session_id, 103, "Missing ip", request_id(&d)).await;
    };

    let mut blacklisted = state.database.blacklisted_ips().await;
    if !blacklisted.iter().any(|entry| entry.ip == ip) {
        return respond_error(state, session_id, 103, "Ip is not blacklisted", request_id(&d)).await;
    }

    blacklisted.retain(|entry| entry.ip != ip);
    if let Err(err) = state.database.set_blacklisted_ips(&blacklisted).await {
        error!("Failed to persist blacklist removal: {}", err);
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 103,
                "d": {
                    "ok": true,
                    "ip": ip
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn admin_broadcast(state: &SharedState, session_id: &str, d: Value) -> bool {
    if !admin_allowed(state, &d) {
        return respond_error(state, session_id, 104, "Unauthorized", request_id(&d)).await;
    }

    let Some(msg) = d.get("msg").and_then(Value::as_str) else {
        return respond_error(state, session_id, 104, "Missing msg", request_id(&d)).await;
    };
    let x = d.get("x").and_then(Value::as_i64).unwrap_or(0);

    let recipients = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| x != 1 || player.version.contains("Ksx"))
            .map(|player| player.tx.clone())
            .collect::<Vec<_>>()
    };

    let payload = json!({
        "op": 87,
        "d": {
            "msg": msg
        }
    });
    let encoded = payload.to_string();
    for recipient in recipients {
        let _ = recipient.send(Message::Text(encoded.clone().into()));
    }

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 104,
                "d": {
                    "ok": true,
                    "msg": msg
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn stats_query(state: &SharedState, session_id: &str, d: Value) -> bool {
    let game_id = d.get("gameId").and_then(Value::as_str);
    let count = match game_id {
        Some(game_id) => room_usernames(state, game_id).await.len(),
        None => {
            let players = state.players.read().await;
            players.len()
        }
    };

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 105,
                "d": {
                    "ok": true,
                    "count": count,
                    "gameId": game_id
                }
            }),
            request_id(&d),
        ),
    )
    .await;
    false
}

async fn respond_error(
    state: &SharedState,
    session_id: &str,
    op: u16,
    error_message: &str,
    req_id: Option<String>,
) -> bool {
    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": op,
                "d": {
                    "error": error_message
                }
            }),
            req_id,
        ),
    )
    .await;
    false
}

async fn respond_to_sender(state: &SharedState, session_id: &str, payload: Value) {
    let tx = {
        let players = state.players.read().await;
        players.get(session_id).map(|player| player.tx.clone())
    };

    if let Some(tx) = tx {
        send_json(&tx, payload);
    }
}

async fn resolve_room_for_session(
    state: &SharedState,
    session_id: &str,
    requested_room: Option<&str>,
) -> Result<String, String> {
    if let Some(room) = requested_room.map(str::trim).filter(|room| !room.is_empty()) {
        return Ok(room.to_owned());
    }

    let players = state.players.read().await;
    if let Some(player) = players.get(session_id) {
        if let Some(first) = player.rooms.iter().next() {
            Ok(first.clone())
        } else {
            Err("Not a member of any room".to_owned())
        }
    } else {
        Err("You need to be identified before".to_owned())
    }
}

async fn dispatch_room_history(
    state: &SharedState,
    session_id: &str,
    room_id: &str,
    req_id: Option<String>,
) {
    let messages = {
        let room_messages = state.room_messages.read().await;
        room_messages.get(room_id).cloned().unwrap_or_default()
    };

    respond_to_sender(
        state,
        session_id,
        with_request_id(
            json!({
                "op": 18,
                "d": {
                    "ok": true,
                    "roomId": room_id,
                    "messages": messages
                }
            }),
            req_id,
        ),
    )
    .await;
}

async fn store_room_message(
    state: &SharedState,
    room_id: &str,
    message: ChatMessageRecord,
) -> ChatMessageRecord {
    let mut rooms = state.room_messages.write().await;
    let room = rooms.entry(room_id.to_owned()).or_default();
    room.push(message.clone());
    if room.len() > MAX_ROOM_MESSAGES {
        let overflow = room.len() - MAX_ROOM_MESSAGES;
        room.drain(0..overflow);
    }
    message
}

async fn update_message_reactions(
    state: &SharedState,
    message_id: &str,
    emoji: &str,
    username: &str,
    room_hint: Option<&str>,
) -> Option<(String, Vec<MessageReaction>)> {
    let mut rooms = state.room_messages.write().await;

    if let Some(room_id) = room_hint.map(str::trim).filter(|room| !room.is_empty()) {
        if let Some(messages) = rooms.get_mut(room_id) {
            if let Some(message) = messages.iter_mut().find(|message| message.message_id == message_id) {
                toggle_reaction_in_message(message, emoji, username);
                return Some((room_id.to_owned(), message.reactions.clone()));
            }
        }
    }

    for (room_id, messages) in rooms.iter_mut() {
        if let Some(message) = messages.iter_mut().find(|message| message.message_id == message_id) {
            toggle_reaction_in_message(message, emoji, username);
            return Some((room_id.clone(), message.reactions.clone()));
        }
    }

    None
}

fn toggle_reaction_in_message(message: &mut ChatMessageRecord, emoji: &str, username: &str) {
    if let Some(index) = message.reactions.iter().position(|reaction| reaction.emoji == emoji) {
        if message.reactions[index].users.iter().any(|user| user == username) {
            message.reactions[index].users.retain(|user| user != username);
        } else {
            message.reactions[index].users.push(username.to_owned());
            message.reactions[index].users.sort();
        }

        message.reactions[index].count = message.reactions[index].users.len();
        if message.reactions[index].count == 0 {
            message.reactions.remove(index);
        }
    } else {
        message.reactions.push(MessageReaction {
            emoji: emoji.to_owned(),
            users: vec![username.to_owned()],
            count: 1,
        });
        message.reactions.sort_by(|a, b| a.emoji.cmp(&b.emoji));
    }
}

async fn room_usernames(state: &SharedState, game_id: &str) -> Vec<String> {
    let players = state.players.read().await;
    players
        .values()
        .filter(|player| player.rooms.contains(game_id))
        .map(|player| player.username.clone())
        .collect()
}

async fn room_voice_usernames(state: &SharedState, game_id: &str) -> Vec<String> {
    let players = state.players.read().await;
    players
        .values()
        .filter(|player| player.rooms.contains(game_id) && player.is_voice_chat)
        .map(|player| player.username.clone())
        .collect()
}

pub async fn broadcast_to_room(state: &SharedState, game_id: &str, payload: Value) {
    let recipients = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| player.rooms.contains(game_id))
            .map(|player| player.tx.clone())
            .collect::<Vec<_>>()
    };

    let encoded = payload.to_string();
    for recipient in recipients {
        let _ = recipient.send(Message::Text(encoded.clone().into()));
    }
}

fn parse_attachment(raw: Option<&Value>) -> Result<Option<Attachment>, &'static str> {
    let Some(obj) = raw else {
        return Ok(None);
    };
    if obj.is_null() {
        return Ok(None);
    }

    let obj = obj.as_object().ok_or("Attachment must be an object")?;

    let data_b64 = obj
        .get("dataB64")
        .and_then(Value::as_str)
        .ok_or("Attachment missing dataB64")?;

    if data_b64.is_empty() {
        return Err("Attachment dataB64 is empty");
    }
    if data_b64.len() > MAX_ATTACHMENT_B64_LEN {
        return Err("Attachment too large (10MB max)");
    }

    let decoded = B64
        .decode(data_b64.as_bytes())
        .map_err(|_| "Attachment dataB64 is not valid base64")?;
    if decoded.len() > MAX_ATTACHMENT_BYTES {
        return Err("Attachment too large (10MB max)");
    }

    let filename = obj
        .get("filename")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("file")
        .chars()
        .take(MAX_FILENAME_LEN)
        .collect::<String>();

    let mime_type = obj
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mt| !mt.is_empty())
        .unwrap_or("application/octet-stream")
        .chars()
        .take(MAX_MIMETYPE_LEN)
        .collect::<String>();

    Ok(Some(Attachment {
        filename,
        mime_type,
        size: decoded.len() as u64,
        data_b64: data_b64.to_owned(),
    }))
}

pub async fn broadcast_to_exchange_key(state: &SharedState, exchange_key: &str, payload: Value) {
    broadcast_to_exchange_key_excluding(state, exchange_key, "", payload).await;
}

pub async fn broadcast_to_exchange_key_excluding(
    state: &SharedState,
    exchange_key: &str,
    excluded_session_id: &str,
    payload: Value,
) {
    let recipients = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| {
                player.exchange_key.as_deref() == Some(exchange_key) && player.id != excluded_session_id
            })
            .map(|player| player.tx.clone())
            .collect::<Vec<_>>()
    };

    let encoded = payload.to_string();
    for recipient in recipients {
        let _ = recipient.send(Message::Text(encoded.clone().into()));
    }
}
