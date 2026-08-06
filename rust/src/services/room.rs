use std::path::Path;

use axum::extract::Multipart;
use serde_json::json;
use tokio::fs;

use crate::{
    core::{
        database::AuthenticatedUser,
        models::{RoomIcon, RoomRecord},
        presence::SharedState,
        result::{ApiError, ApiResult},
    },
    services::user::store_uploaded_bytes,
    websocket::protocol,
};

const MAX_ROOM_ICON_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

pub async fn upload_room_icon(
    state: &SharedState,
    user: &AuthenticatedUser,
    room_id: &str,
    mut multipart: Multipart,
) -> ApiResult<serde_json::Value> {
    let room_id = room_id.trim().to_owned();
    if room_id.is_empty() {
        return Err(ApiError::bad_request("Invalid room."));
    }

    let is_member = {
        let players = state.players.read().await;
        players
            .values()
            .filter(|player| player.user_id == user.id)
            .any(|player| player.rooms.contains(&room_id))
    };
    if !is_member {
        return Err(ApiError::forbidden("You are not in this room."));
    }

    let mut file_name = String::new();
    let mut declared_mime = String::new();
    let mut bytes = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name().unwrap_or_default() == "file" {
            file_name = field.file_name().unwrap_or("room-icon").to_owned();
            declared_mime = field.content_type().unwrap_or_default().to_owned();
            bytes = field
                .bytes()
                .await
                .map(|val| val.to_vec())
                .unwrap_or_default();
        }
    }
    if bytes.is_empty() {
        return Err(ApiError::bad_request("Missing file."));
    }
    if bytes.len() > MAX_ROOM_ICON_UPLOAD_BYTES {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "Room icon too large.",
        ));
    }

    let (mime_type, _, _) = protocol::detect_profile_image(&bytes, &declared_mime)
        .map_err(ApiError::bad_request)?;
    let extension = file_extension(&file_name, mime_type);

    if let Some(previous_icon) = state.database.room_icon(&room_id).await {
        let previous_path = Path::new(&state.config.network.upload_dir).join(&previous_icon.file.id);
        let _ = fs::remove_file(previous_path).await;
    }

    let stored = store_uploaded_bytes(state, extension, &bytes, mime_type).await?;
    let icon = RoomIcon { file: stored };
    state.database.set_room_icon(&room_id, &icon).await?;
    protocol::sync_room_record(state, &room_id)
        .await
        .map_err(|err| ApiError::internal("Sync room record", err))?;

    let room = state
        .database
        .room_record(&room_id)
        .await
        .unwrap_or(RoomRecord {
            room_id: room_id.clone(),
            title: room_id.clone(),
            icon: Some(icon.clone()),
            members: protocol::room_usernames(state, &room_id, None).await,
        });

    protocol::broadcast_to_room(
        state,
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

    Ok(json!({ "ok": true, "icon": icon, "room": room }))
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
