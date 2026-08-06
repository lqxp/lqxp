use axum::extract::Multipart;
use serde_json::json;

use crate::{
    core::{
        database::AuthenticatedUser,
        models::{ProfileImage, UserPresenceStatus, UserProfile},
        presence::SharedState,
        result::{ApiError, ApiResult},
    },
    websocket::protocol,
};

const MAX_PROFILE_AVATAR_BYTES: usize = 10 * 1024 * 1024;
const MAX_PROFILE_BANNER_BYTES: usize = 15 * 1024 * 1024;

pub async fn upload_profile_image(
    state: &SharedState,
    user: &AuthenticatedUser,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
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
            bytes = field
                .bytes()
                .await
                .map(|val| val.to_vec())
                .unwrap_or_default();
        }
    }

    let kind = kind.trim();
    let max_bytes = match kind {
        "avatar" => MAX_PROFILE_AVATAR_BYTES,
        "banner" => MAX_PROFILE_BANNER_BYTES,
        _ => return Err(ApiError::bad_request("Invalid profile image kind.")),
    };
    if bytes.is_empty() {
        return Err(ApiError::bad_request("Missing file."));
    }
    if bytes.len() > max_bytes {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "Profile image too large.",
        ));
    }

    let (mime_type, width, height) = protocol::detect_profile_image(&bytes, &declared_mime)
        .map_err(ApiError::bad_request)?;
    let extension = file_extension(&file_name, mime_type);
    let stored = store_uploaded_bytes(state, extension, &bytes, mime_type).await?;
    let image = ProfileImage {
        file: stored,
        width,
        height,
    };

    let (profile, sessions) = {
        let mut players = state.players.write().await;
        let mut profile = None;
        let mut sessions = Vec::new();
        for player in players.values_mut().filter(|p| p.user_id == user.id) {
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
        let mut profile = profile.unwrap_or_else(|| user.profile.clone());
        match kind {
            "avatar" => profile.avatar = Some(image.clone()),
            "banner" => profile.banner = Some(image.clone()),
            _ => {}
        }
        (profile, sessions)
    };

    state.accounts.update_profile(&user.id, &profile).await?;

    broadcast_profile_update(state, sessions, profile.clone()).await;

    Ok(json!({ "ok": true, "profile": profile, kind: image }))
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

pub async fn store_uploaded_bytes(
    state: &crate::core::presence::AppState,
    extension: &str,
    bytes: &[u8],
    mime_type: &str,
) -> ApiResult<crate::core::models::StoredFile> {
    let file_id = if extension.trim().is_empty() {
        uuid::Uuid::new_v4().simple().to_string()
    } else {
        format!(
            "{}.{}",
            uuid::Uuid::new_v4().simple(),
            extension.trim().trim_start_matches('.')
        )
    };
    let path = std::path::Path::new(&state.config.network.upload_dir).join(&file_id);
    std::fs::write(&path, bytes)
        .map_err(|err| ApiError::internal("Failed to store file", err))?;
    Ok(crate::core::models::StoredFile {
        id: file_id.clone(),
        url: public_file_url(state, &file_id),
        size: bytes.len() as u64,
        mime_type: mime_type.to_owned(),
    })
}

pub fn public_file_url(state: &crate::core::presence::AppState, file_id: &str) -> String {
    let base = state.config.network.upload_public_base.trim();
    let base = if base.is_empty() { "/uploads" } else { base };
    let base = if base.starts_with('/') {
        base.trim_end_matches('/').to_owned()
    } else {
        format!("/{}", base.trim_matches('/'))
    };
    format!("{}/{}", base, file_id)
}

async fn broadcast_profile_update(
    state: &SharedState,
    sessions: Vec<(String, UserPresenceStatus, String, String, Vec<String>)>,
    profile: UserProfile,
) {
    for (username, status, client_id, platform, rooms) in sessions {
        if status == UserPresenceStatus::Invisible {
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
