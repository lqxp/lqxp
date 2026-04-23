use std::{net::SocketAddr, path::{Path, PathBuf}};

use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::fs;

use crate::{state::SharedState, utils::extract_client_ip, websocket::handle_socket};

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(webchat_page))
        .route("/ws", get(ws_upgrade))
        .route("/*path", get(public_asset))
        .with_state(state)
}

async fn webchat_page(State(state): State<SharedState>) -> impl IntoResponse {
    let path = PathBuf::from(&state.config.network.public_dir).join(&state.config.network.webchat_index);
    serve_file(&path).await
}

async fn public_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let sanitized = path.trim_start_matches('/');
    let full_path = PathBuf::from(&state.config.network.public_dir).join(sanitized);
    serve_file(&full_path).await
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
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!("Resource not found: {}", path.display()),
        )
            .into_response(),
    }
}

async fn ws_upgrade(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let ip = extract_client_ip(&headers, addr);
    ws.max_frame_size(32 * 1024 * 1024)
        .max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| handle_socket(state, socket, ip))
}
