use std::collections::{BTreeMap, HashSet};

use serde_json::json;

use crate::{
    core::{
        database::{AuthenticatedUser, PublicUser},
        presence::SharedState,
        result::{ApiError, ApiResult},
    },
    websocket::protocol,
};

pub async fn admin_overview(
    state: &SharedState,
    admin: &AuthenticatedUser,
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }

    let users = state.accounts.list_users().await?;
    let features = state.accounts.feature_flags().await?;

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

    Ok(json!({
        "ok": true,
        "users": users,
        "features": features,
        "rooms": rooms,
        "onlineCount": online_count
    }))
}

pub async fn set_feature(
    state: &SharedState,
    admin: &AuthenticatedUser,
    raw_key: &str,
    enabled: bool,
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }
    let key = match raw_key {
        "registerEnabled" | "register_enabled" => "register_enabled",
        "callsEnabled" | "calls_enabled" => "calls_enabled",
        _ => return Err(ApiError::bad_request("Unknown feature.")),
    };
    state.accounts.set_feature(key, enabled).await?;
    let features = state.accounts.feature_flags().await?;
    Ok(json!({ "ok": true, "features": features }))
}

pub async fn set_user_disabled(
    state: &SharedState,
    admin: &AuthenticatedUser,
    target_user_id: &str,
    disabled: bool,
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }
    if admin.id == target_user_id {
        return Err(ApiError::bad_request("You cannot disable your own account."));
    }
    state
        .accounts
        .set_user_disabled(target_user_id, disabled)
        .await?;
    Ok(json!({ "ok": true }))
}

pub async fn set_user_banned(
    state: &SharedState,
    admin: &AuthenticatedUser,
    target_user_id: &str,
    banned: bool,
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }
    if admin.id == target_user_id {
        return Err(ApiError::bad_request("You cannot ban your own account."));
    }
    state.accounts.set_user_banned(target_user_id, banned).await?;
    if banned {
        state.evict_banned_user(target_user_id).await;
    }
    Ok(json!({ "ok": true }))
}

pub async fn delete_user_account(
    state: &SharedState,
    admin: &AuthenticatedUser,
    target_user_id: &str,
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }
    if admin.id == target_user_id {
        return Err(ApiError::bad_request("You cannot delete your own account."));
    }
    state.accounts.delete_user_account(target_user_id).await?;
    disconnect_user_sessions(state, target_user_id).await;
    Ok(json!({ "ok": true }))
}

pub async fn set_user_badges(
    state: &SharedState,
    admin: &AuthenticatedUser,
    target_user_id: &str,
    badges: &[String],
) -> ApiResult<serde_json::Value> {
    if !admin.admin {
        return Err(ApiError::forbidden("Admin only."));
    }
    let updated_user = state
        .accounts
        .set_user_badges(target_user_id, badges)
        .await?;
    broadcast_badge_update(state, &updated_user).await;
    Ok(json!({ "ok": true, "user": updated_user }))
}

async fn disconnect_user_sessions(state: &SharedState, user_id: &str) {
    let sessions: Vec<(String, tokio::sync::mpsc::UnboundedSender<axum::extract::ws::Message>)> = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| player.user_id == user_id)
            .map(|player| (player.id.clone(), player.tx.clone()))
            .collect()
    };

    for (session_id, tx) in sessions {
        let _ = tx.send(axum::extract::ws::Message::Text(
            json!({ "op": 0, "d": { "error": "Account deleted." } })
                .to_string()
                .into(),
        ));
        let _ = tx.send(axum::extract::ws::Message::Close(None));
        crate::websocket::disconnect_player(state, &session_id).await;
    }
}

async fn broadcast_badge_update(state: &SharedState, user: &PublicUser) {
    let mut touched_rooms: HashSet<String> = HashSet::new();
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
        let _ = tx.send(axum::extract::ws::Message::Text(
            json!({
                "op": 34,
                "d": {
                    "ok": true,
                    "user": user.username,
                    "username": user.username,
                    "userId": user.id,
                    "badges": user.badges
                }
            })
            .to_string()
            .into(),
        ));
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
            let _ = tx.send(axum::extract::ws::Message::Text(
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
                })
                .to_string()
                .into(),
            ));
        }
    }
}
