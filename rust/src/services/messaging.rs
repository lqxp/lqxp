use std::path::Path;

use crate::core::presence::SharedState;

pub async fn upload_is_live(state: &SharedState, raw_path: &str) -> bool {
    let Some(file_id) = Path::new(raw_path.trim_start_matches('/'))
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
    else {
        return false;
    };

    {
        let rooms = state.room_messages.read().await;
        if rooms.values().any(|messages| {
            messages
                .iter()
                .filter(|message| !message.deleted)
                .any(|message| message.attachment.as_ref().is_some_and(|att| att.id == file_id))
        }) {
            return true;
        }
    }

    if state
        .accounts
        .profile_uses_file_id(&file_id)
        .await
        .unwrap_or(false)
    {
        return true;
    }

    state
        .database
        .room_icon_uses_file_id(&file_id)
        .await
        .unwrap_or(false)
}
