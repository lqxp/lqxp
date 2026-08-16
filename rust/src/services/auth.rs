use std::collections::{BTreeMap, HashSet};

use serde_json::json;

use crate::core::{
    database::{AuthenticatedUser, PublicUser},
    presence::SharedState,
    result::{ApiError, ApiResult},
    security::username_hits_blocklist,
};

pub fn user_response(user: PublicUser, token: String) -> serde_json::Value {
    json!({
        "ok": true,
        "token": token,
        "user": user
    })
}

pub async fn register(
    state: &SharedState,
    username: &str,
    password: &str,
) -> ApiResult<serde_json::Value> {
    let flags = state.accounts.feature_flags().await?;
    if !flags.register_enabled {
        return Err(ApiError::forbidden("Registrations are disabled."));
    }
    if username_hits_blocklist(username, &state.blocklist_terms) {
        return Err(ApiError::bad_request("Username is not allowed."));
    }

    let (user, token, recovery_words) = state.accounts.register(username, password).await?;
    Ok(json!({
        "ok": true,
        "token": token,
        "user": user,
        "recoveryWords": recovery_words
    }))
}

pub async fn login(
    state: &SharedState,
    username: &str,
    password: &str,
) -> ApiResult<serde_json::Value> {
    let (user, token) = state.accounts.login(username, password).await?;
    Ok(user_response(user, token))
}

pub async fn recover(
    state: &SharedState,
    username: &str,
    recovery_words: &str,
    new_password: &str,
) -> ApiResult<serde_json::Value> {
    let (user, token) = state
        .accounts
        .recover(username, recovery_words, new_password)
        .await?;
    Ok(user_response(user, token))
}

pub async fn me(state: &SharedState, token: &str) -> ApiResult<serde_json::Value> {
    let result = state.accounts.me(token).await?;
    match result {
        Some((user, token)) => Ok(user_response(user, token)),
        None => Err(ApiError::unauthorized("Invalid session.")),
    }
}

pub async fn logout(state: &SharedState, token: &str) -> ApiResult<()> {
    state.accounts.logout(token).await
}

pub async fn delete_account(state: &SharedState, token: &str, password: &str) -> ApiResult<()> {
    let Some(user) = state.accounts.authenticate_token(token).await? else {
        return Err(ApiError::unauthorized("Not authenticated."));
    };
    state.accounts.delete_account(token, password).await?;
    state.evict_user(&user.id, "Account deleted").await;
    state.invalidate_public_profile_cache(Some(&user.id), Some(&user.username)).await;
    if let Some(avatar) = user.profile.avatar {
        let path = std::path::Path::new(&state.config.network.upload_dir).join(&avatar.file.id);
        let _ = tokio::fs::remove_file(path).await;
    }
    if let Some(banner) = user.profile.banner {
        let path = std::path::Path::new(&state.config.network.upload_dir).join(&banner.file.id);
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(())
}

pub async fn change_username(
    state: &SharedState,
    user: &AuthenticatedUser,
    new_username: &str,
) -> ApiResult<serde_json::Value> {
    if username_hits_blocklist(new_username, &state.blocklist_terms) {
        return Err(ApiError::bad_request("Username is not allowed."));
    }

    let updated_user = state.accounts.change_username(&user.id, new_username).await?;

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
                        && player.status != crate::core::models::UserPresenceStatus::Invisible
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
                .filter(|player| player.is_call_in_room(&room_id))
                .map(|player| player.username.clone())
                .collect::<Vec<_>>();

            let call_players = room_players
                .iter()
                .filter(|player| player.is_call_in_room(&room_id))
                .map(|player| {
                    json!({
                        "username": player.username,
                        "clientId": player.client_id,
                        "platform": player.platform,
                        "isVoiceChat": player.is_voice_chat,
                        "deafened": player.call_deafened,
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
            let _ = tx.send(axum::extract::ws::Message::Text(payload.to_string()));
        }
    }

    state.invalidate_public_profile_cache(Some(&user.id), Some(&user.username)).await;
    state.invalidate_public_profile_cache(Some(&user.id), Some(&updated_user.username)).await;

    Ok(json!({ "ok": true, "user": updated_user }))
}
