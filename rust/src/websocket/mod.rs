pub mod protocol;

use std::collections::HashSet;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{info, warn};

use crate::{
    core::{
        models::{UserPresenceStatus, UserProfile},
        presence::{PlayerSession, SharedState},
        security::rate_limit_hit,
    },
    utils::{random_session_id, send_json},
};

pub async fn handle_socket(state: SharedState, socket: WebSocket) {
    let session_id = random_session_id();
    let (ws_sender, mut ws_receiver) = socket.split();
    let (tx, rx) = mpsc::unbounded_channel::<Message>();

    let writer = spawn_writer_task(ws_sender, rx);

    register_connection(&state, &session_id, tx.clone()).await;

    send_json(
        &tx,
        json!({
            "op": 10,
            "d": {
                "heartbeat_interval": state.config.network.heartbeat_interval
            }
        }),
    );

    info!("Client connected: {}", session_id);

    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                if rate_limit_hit(&state, format!("ws-msg:session:{session_id}"), 120, 60_000).await {
                    send_json(&tx, json!({ "op": 0, "d": { "error": "Rate limited." } }));
                    break;
                }
                let should_close = protocol::process_message(
                    state.clone(),
                    session_id.clone(),
                    tx.clone(),
                    text.to_string(),
                )
                .await;
                if should_close {
                    let _ = tx.send(Message::Close(None));
                    break;
                }
            }
            Ok(Message::Binary(payload)) => {
                if rate_limit_hit(&state, format!("ws-msg:session:{session_id}"), 120, 60_000).await {
                    send_json(&tx, json!({ "op": 0, "d": { "error": "Rate limited." } }));
                    break;
                }
                let text = String::from_utf8_lossy(&payload).into_owned();
                let should_close = protocol::process_message(
                    state.clone(),
                    session_id.clone(),
                    tx.clone(),
                    text,
                )
                .await;
                if should_close {
                    let _ = tx.send(Message::Close(None));
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                warn!("WebSocket error for {}: {}", session_id, err);
                break;
            }
        }
    }

    disconnect_player(&state, &session_id).await;
    writer.abort();
}

fn spawn_writer_task(
    mut ws_sender: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::UnboundedReceiver<Message>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if ws_sender.send(message).await.is_err() {
                break;
            }
        }
    })
}

async fn register_connection(
    state: &SharedState,
    session_id: &str,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut players = state.players.write().await;
    players.insert(
        session_id.to_owned(),
        PlayerSession {
            id: session_id.to_owned(),
            user_id: String::new(),
            is_admin: false,
            badges: Vec::new(),
            username: String::new(),
            tx,
            rooms: HashSet::new(),
            is_voice_chat: false,
            call_room: None,
            call_camera: false,
            call_screen: false,
            call_deafened: false,
            client_id: String::new(),
            platform: "web".to_owned(),
            version: "unknown".to_owned(),
            last_message_timestamp: None,
            is_mobile: None,
            is_secure: None,
            muted_users: HashSet::new(),
            delete_messages_on_leave: false,
            profile: UserProfile::default(),
            status: UserPresenceStatus::Online,
        },
    );
}

pub async fn disconnect_player(state: &SharedState, session_id: &str) {
    let removed = {
        let mut players = state.players.write().await;
        players.remove(session_id)
    };

    let Some(player) = removed else {
        return;
    };

    let username = player.username.clone();

    for game_id in &player.rooms {
        if let Err(err) = protocol::sync_room_record(state, game_id).await {
            warn!("Failed to persist room {} on disconnect: {}", game_id, err);
        }
        if player.status != UserPresenceStatus::Invisible {
            let room_record = state.database.room_record(game_id).await.unwrap_or(
                crate::core::models::RoomRecord {
                    room_id: game_id.clone(),
                    title: game_id.clone(),
                    icon: None,
                    members: Vec::new(),
                },
            );
            protocol::broadcast_to_room(
                state,
                game_id,
                json!({
                    "op": 4,
                    "d": {
                        "gameId": game_id,
                        "left": username.clone(),
                        "clientId": player.client_id.clone(),
                        "room": room_record
                    }
                }),
            )
            .await;
        }

        if player.delete_messages_on_leave {
            protocol::delete_user_messages_in_room_and_broadcast(state, game_id, &username).await;
        }

        if player.status != UserPresenceStatus::Invisible {
            protocol::broadcast_to_room(
                state,
                game_id,
                json!({
                    "op": 98,
                    "d": {
                        "gameId": game_id,
                        "user": username.clone(),
                        "status": player.status,
                        "isVoiceChat": false,
                        "clientId": player.client_id.clone(),
                        "platform": player.platform.clone()
                    }
                }),
            )
            .await;
        }
    }

    info!("Client disconnected: {}", player.id);
}
