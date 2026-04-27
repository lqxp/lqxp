pub mod protocol;

use std::collections::HashSet;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde_json::{json, Value};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{info, warn};

use crate::{
    models::UserProfile,
    state::{PlayerSession, SharedState},
    utils::{random_session_id, send_json},
};

pub async fn handle_socket(state: SharedState, socket: WebSocket, ip: String) {
    let session_id = random_session_id();
    let (ws_sender, mut ws_receiver) = socket.split();
    let (tx, rx) = mpsc::unbounded_channel::<Message>();

    let writer = spawn_writer_task(ws_sender, rx);

    if let Err(reason) = register_connection(&state, &session_id, &ip, tx.clone()).await {
        send_json(&tx, reason);
        let _ = tx.send(Message::Close(None));
        let _ = writer.await;
        return;
    }

    send_json(
        &tx,
        json!({
            "op": 10,
            "d": {
                "heartbeat_interval": state.config.network.heartbeat_interval
            }
        }),
    );

    info!("Client connected: {} ({})", session_id, ip);

    while let Some(result) = ws_receiver.next().await {
        match result {
            Ok(Message::Text(text)) => {
                let should_close = protocol::process_message(
                    state.clone(),
                    session_id.clone(),
                    ip.clone(),
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
                let text = String::from_utf8_lossy(&payload).into_owned();
                let should_close = protocol::process_message(
                    state.clone(),
                    session_id.clone(),
                    ip.clone(),
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
    ip: &str,
    tx: mpsc::UnboundedSender<Message>,
) -> Result<(), Value> {
    {
        let mut ip_connections = state.ip_connections.write().await;
        let current = ip_connections.entry(ip.to_owned()).or_insert(0);
        *current += 1;

        if *current > state.config.network.max_connections_per_ip {
            *current -= 1;
            return Err(json!({
                "op": 0,
                "d": {
                    "error": "Too many connections from this IP."
                }
            }));
        }
    }

    let blacklisted = state.database.blacklisted_ips().await;
    if let Some(entry) = blacklisted.iter().find(|entry| entry.ip == ip) {
        decrement_ip_connection(state, ip).await;
        return Err(json!({
            "op": 24,
            "d": {
                "error": "You are blacklisted.",
                "reason": entry.reason,
                "timestamp": entry.timestamp,
                "ign": entry.ign
            }
        }));
    }

    let mut players = state.players.write().await;
    players.insert(
        session_id.to_owned(),
        PlayerSession {
            id: session_id.to_owned(),
            ip: ip.to_owned(),
            username: String::new(),
            tx,
            rooms: HashSet::new(),
            is_voice_chat: false,
            call_camera: false,
            call_screen: false,
            version: "unknown".to_owned(),
            last_message_timestamp: None,
            last_voice_chunk_timestamp: None,
            exchange_key: None,
            is_mobile: None,
            is_secure: None,
            muted_users: HashSet::new(),
            delete_messages_on_leave: false,
            profile: UserProfile::default(),
        },
    );

    Ok(())
}

pub async fn decrement_ip_connection(state: &SharedState, ip: &str) {
    let mut ip_connections = state.ip_connections.write().await;
    if let Some(count) = ip_connections.get_mut(ip) {
        if *count > 1 {
            *count -= 1;
        } else {
            ip_connections.remove(ip);
        }
    }
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
        protocol::broadcast_to_room(
            state,
            game_id,
            json!({
                "op": 4,
                "d": {
                    "gameId": game_id,
                    "left": username.clone()
                }
            }),
        )
        .await;

        if player.delete_messages_on_leave {
            protocol::delete_user_messages_in_room_and_broadcast(state, game_id, &username).await;
        }

        protocol::broadcast_to_room(
            state,
            game_id,
            json!({
                "op": 98,
                "d": {
                    "gameId": game_id,
                    "user": username.clone(),
                    "isVoiceChat": false
                }
            }),
        )
        .await;
    }

    if let Some(exchange_key) = &player.exchange_key {
        protocol::broadcast_to_exchange_key(
            state,
            exchange_key,
            json!({
                "op": 14,
                "d": {
                    "username": player.username.clone()
                }
            }),
        )
        .await;
    }

    decrement_ip_connection(state, &player.ip).await;
    info!("Client disconnected: {} ({})", player.id, player.ip);
}
