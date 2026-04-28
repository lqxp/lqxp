mod accounts;
mod config;
mod db;
mod linkpreview;
mod models;
mod server;
mod state;
mod utils;
mod websocket;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use tokio::{net::TcpListener, sync::RwLock};
use tracing::info;

use crate::{accounts::AccountDatabase, config::{init_tracing, load_blocklist_terms, load_config}, db::JsonDatabase, server::build_router, state::AppState};

#[tokio::main]
async fn main() {
    init_tracing();

    let config = load_config().await;
    let blocklist_terms = load_blocklist_terms().await;
    let database = Arc::new(JsonDatabase::load(PathBuf::from("files/database.json")).await);
    let accounts = Arc::new(
        AccountDatabase::connect(
            &config.database,
            config.security.admin_ids.clone(),
            config.security.register_enabled,
        )
        .await
        .expect("failed to initialize account database"),
    );

    let state = Arc::new(AppState {
        config: config.clone(),
        blocklist_terms: Arc::new(blocklist_terms),
        players: Arc::new(RwLock::new(HashMap::new())),
        ip_connections: Arc::new(RwLock::new(HashMap::new())),
        room_messages: Arc::new(RwLock::new(HashMap::new())),
        database,
        accounts,
    });

    let app = build_router(state.clone());

    let bind_host = if config.api.domain.trim().is_empty() {
        "0.0.0.0".to_owned()
    } else {
        config.api.domain.clone()
    };

    let listener = TcpListener::bind((bind_host.as_str(), config.api.port))
        .await
        .expect("failed to bind TCP listener");

    info!(
        "QxProtocol Rust server listening on {}:{}",
        bind_host, config.api.port
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("server exited unexpectedly");
}
