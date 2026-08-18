use std::{
    net::IpAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{
        ws::WebSocketUpgrade, DefaultBodyLimit, Multipart, Path as AxumPath, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::fs;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{
    core::{
        database::AuthenticatedUser,
        presence::SharedState,
        result::{ApiError, ApiResult},
    },
    services::{admin, auth, messaging, room, user},
    websocket::handle_socket,
};

pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/app", get(webchat_page))
        .route("/app/", get(webchat_page))
        .route("/app/uploads/*path", get(upload_asset))
        .route("/app/*path", get(app_asset))
        .route("/api/auth/challenge", get(auth_challenge_handler))
        .route("/api/auth/cap/challenge", get(cap_challenge_handler))
        .route("/api/auth/cap/redeem", post(cap_redeem_handler))
        .route("/api/auth/me", get(auth_me_handler))
        .route("/api/auth/register", post(auth_register_handler))
        .route("/api/auth/login", post(auth_login_handler))
        .route("/api/auth/recover", post(auth_recover_handler))
        .route("/api/auth/logout", post(auth_logout_handler))
        .route("/api/auth/delete", post(auth_delete_handler))
        .route("/api/auth/username", post(auth_username_handler))
        .route("/api/profile/image", post(profile_image_upload_handler))
        .route("/api/rooms/:room_id/icon", post(room_icon_upload_handler))
        .route("/api/admin/overview", get(admin_overview_handler))
        .route("/api/admin/features", post(admin_features_handler))
        .route(
            "/api/admin/users/:user_id/disabled",
            post(admin_user_disabled_handler),
        )
        .route(
            "/api/admin/users/:user_id/banned",
            post(admin_user_banned_handler),
        )
        .route(
            "/api/admin/users/:user_id/delete",
            post(admin_user_delete_handler),
        )
        .route(
            "/api/admin/users/:user_id/badges",
            post(admin_user_badges_handler),
        )
        .route("/api/admin/users/purge", post(admin_users_purge_handler))
        .route("/api/release", get(latest_release_handler))
        .route("/ws", get(ws_upgrade_handler))
        .route("/*path", get(public_asset_handler))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(cors_layer(&state))
        .with_state(state)
}

fn cors_layer(state: &SharedState) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(allowed_cors_origins(state))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

fn allowed_cors_origins(state: &SharedState) -> AllowOrigin {
    let configured_origins: Vec<HeaderValue> = [
        state.config.api.public_domain.trim(),
        state.config.api.domain.trim(),
    ]
    .into_iter()
    .filter(|o| !o.is_empty())
    .flat_map(|o| {
        if o.starts_with("http://") || o.starts_with("https://") {
            vec![o.to_owned()]
        } else {
            vec![format!("https://{o}"), format!("http://{o}")]
        }
    })
    .filter_map(|o| o.parse().ok())
    .collect();

    AllowOrigin::predicate(move |origin: &HeaderValue, _parts: &axum::http::request::Parts| {
        let s = origin.to_str().unwrap_or("");

        if configured_origins.iter().any(|o| o.as_bytes() == origin.as_bytes()) {
            return true;
        }

        if s == "tauri://localhost"
            || s == "http://tauri.localhost"
            || s == "https://tauri.localhost"
        {
            return true;
        }

        if s.starts_with("http://localhost:")
            || s.starts_with("http://127.0.0.1:")
            || s.starts_with("http://[::1]:")
        {
            return true;
        }

        // Allow any HTTP origin from a private / loopback IP
        if let Ok(parsed) = url::Url::parse(s) {
            if parsed.scheme() == "http" {
                if let Some(host) = parsed.host_str() {
                    if host.eq_ignore_ascii_case("localhost") {
                        return true;
                    }
                    if let Ok(ip) = host.parse::<IpAddr>() {
                        return match ip {
                            IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
                            IpAddr::V6(v6) => v6.is_loopback(),
                        };
                    }
                }
            }
        }

        false
    })
}

#[derive(Debug, Deserialize)]
struct ChallengeQuery {
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapChallengeQuery {
    scope: Option<String>,
    target: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChallengeResponse {
    vdf: crate::core::vdf::VdfChallenge,
    quota_token: crate::core::rln::EpochQuotaToken,
    pqc_key: crate::core::pqc::PqcPublicKey,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRegisterRequest {
    username: String,
    password: String,
    #[serde(alias = "cap_token")]
    cap_token: Option<String>,
    vdf_challenge: Option<crate::core::vdf::VdfChallenge>,
    vdf_proof: Option<crate::core::vdf::VdfProof>,
    quota_token: Option<crate::core::rln::EpochQuotaToken>,
    nullifier: Option<String>,
    pqc_ciphertext: Option<crate::core::pqc::PqcCiphertext>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthLoginRequest {
    username: String,
    password: String,
    #[serde(alias = "cap_token")]
    cap_token: Option<String>,
    vdf_challenge: Option<crate::core::vdf::VdfChallenge>,
    vdf_proof: Option<crate::core::vdf::VdfProof>,
    quota_token: Option<crate::core::rln::EpochQuotaToken>,
    nullifier: Option<String>,
    pqc_ciphertext: Option<crate::core::pqc::PqcCiphertext>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRecoverRequest {
    username: String,
    recovery_words: String,
    new_password: String,
    #[serde(alias = "cap_token")]
    cap_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthDeleteRequest {
    password: String,
}

#[derive(Debug, Deserialize)]
struct UsernameRequest {
    username: String,
}

#[derive(Debug, Deserialize)]
struct FeatureRequest {
    key: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct DisabledRequest {
    disabled: bool,
}

#[derive(Debug, Deserialize)]
struct BannedRequest {
    banned: bool,
}

#[derive(Debug, Deserialize)]
struct BadgesRequest {
    badges: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PurgeUsersRequest {
    created_after_ms: Option<u64>,
    created_before_ms: Option<u64>,
    min_username_len: Option<usize>,
    max_username_len: Option<usize>,
    username_contains: Option<String>,
}

async fn cap_challenge_handler(
    State(state): State<SharedState>,
    Query(query): Query<CapChallengeQuery>,
) -> ApiResult<impl IntoResponse> {
    if crate::core::security::rate_limit_hit(&state, "auth:cap:challenge:global".to_string(), 60, 10_000).await {
        return Err(ApiError::too_many_requests(
            "CAPTCHA challenge rate limit exceeded. Please wait a few seconds.",
        ));
    }

    let scope = query.scope.as_deref().unwrap_or("register");
    let target = query.target.as_deref();
    let challenge = crate::core::cap::generate_cap_challenge(scope, target).await;

    Ok(Json(challenge))
}

async fn cap_redeem_handler(
    State(state): State<SharedState>,
    Json(body): Json<crate::core::cap::CapRedeemRequest>,
) -> ApiResult<impl IntoResponse> {
    if crate::core::security::rate_limit_hit(&state, "auth:cap:redeem:global".to_string(), 60, 10_000).await {
        return Err(ApiError::too_many_requests(
            "CAPTCHA redeem rate limit exceeded. Please wait a few seconds.",
        ));
    }

    let res = crate::core::cap::redeem_cap_challenge(body).await?;
    Ok(Json(res))
}

async fn auth_challenge_handler(
    State(state): State<SharedState>,
    Query(query): Query<ChallengeQuery>,
) -> ApiResult<impl IntoResponse> {
    if crate::core::security::rate_limit_hit(&state, "auth:challenge:global".to_string(), 60, 10_000).await {
        return Err(ApiError::too_many_requests(
            "Security challenge rate limit exceeded. Please wait a few seconds.",
        ));
    }

    let target = query.target.as_deref();
    let vdf = crate::core::vdf::generate_vdf_challenge(target, None);
    let quota_token = crate::core::rln::generate_quota_token();
    let pqc_key = crate::core::pqc::issue_pqc_challenge().await;

    Ok(Json(ChallengeResponse {
        vdf,
        quota_token,
        pqc_key,
    }))
}

async fn auth_register_handler(
    State(state): State<SharedState>,
    Json(body): Json<AuthRegisterRequest>,
) -> ApiResult<impl IntoResponse> {
    let clean_user = crate::core::security::normalize_username(&body.username);

    // Global registration rate limit: prevent automated mass account creation burst
    if crate::core::security::rate_limit_hit(&state, "register:global".to_string(), 10, 15_000).await {
        return Err(ApiError::too_many_requests(
            "Server registration capacity reached. Please wait 15 seconds.",
        ));
    }

    // Per-username rate limit
    let rate_key = format!("register:user:{}", clean_user);
    if crate::core::security::rate_limit_hit(&state, rate_key, 3, 30_000).await {
        return Err(ApiError::too_many_requests(
            "Too many registration attempts for this username. Please wait 30 seconds.",
        ));
    }

    // 1. Validate Cap token if provided
    if let Some(token) = &body.cap_token {
        crate::core::cap::verify_and_consume_cap_token(token, "register").await?;
    } else {
        // Fallback to legacy challenge verification
        let quota_token = body.quota_token.as_ref().ok_or_else(|| {
            ApiError::bad_request("Missing CAPTCHA or quota token. Please solve the security challenge.")
        })?;
        let nullifier = body.nullifier.as_deref().ok_or_else(|| {
            ApiError::bad_request("Missing rate-limiting nullifier.")
        })?;
        crate::core::rln::verify_and_consume_nullifier(quota_token, nullifier, "register").await?;

        let (vdf_c, vdf_p) = match (&body.vdf_challenge, &body.vdf_proof) {
            (Some(c), Some(p)) => (c, p),
            _ => {
                return Err(ApiError::bad_request(
                    "Missing Verifiable Delay Function (VDF) proof. Please solve the security challenge.",
                ))
            }
        };
        crate::core::vdf::verify_and_consume_vdf(vdf_c, vdf_p, Some(&body.username)).await?;

        let pqc_ct = body.pqc_ciphertext.as_ref().ok_or_else(|| {
            ApiError::bad_request("Missing Post-Quantum KEM ciphertext. Please solve the security challenge.")
        })?;
        let _pqc_shared_secret = crate::core::pqc::verify_and_decapsulate_pqc(pqc_ct).await?;
    }

    auth::register(&state, &body.username, &body.password)
        .await
        .map(Json)
}

async fn auth_login_handler(
    State(state): State<SharedState>,
    Json(body): Json<AuthLoginRequest>,
) -> ApiResult<impl IntoResponse> {
    let clean_user = crate::core::security::normalize_username(&body.username);
    let fail_key = format!("login:fail:user:{}", clean_user);

    {
        let buckets = state.rate_limits.lock().await;
        if let Some(bucket) = buckets.get(&fail_key) {
            let now = crate::core::models::now_ms();
            if now.saturating_sub(bucket.window_start_ms) <= 30_000 && bucket.count >= 5 && body.cap_token.is_none() && body.vdf_proof.is_none() {
                return Err(ApiError::too_many_requests(
                    "Too many failed login attempts for this account. Please solve the security challenge to proceed.",
                ));
            }
        }
    }

    if let Some(token) = &body.cap_token {
        crate::core::cap::verify_and_consume_cap_token(token, "login").await?;
    } else {
        let quota_token = body.quota_token.as_ref().ok_or_else(|| {
            ApiError::bad_request("Missing CAPTCHA or quota token. Please solve the security verification.")
        })?;
        let nullifier = body.nullifier.as_deref().ok_or_else(|| {
            ApiError::bad_request("Missing rate-limiting nullifier.")
        })?;
        crate::core::rln::verify_and_consume_nullifier(quota_token, nullifier, "login").await?;

        let (vdf_c, vdf_p) = match (&body.vdf_challenge, &body.vdf_proof) {
            (Some(c), Some(p)) => (c, p),
            _ => {
                return Err(ApiError::bad_request(
                    "Missing Verifiable Delay Function (VDF) proof. Please solve the security verification.",
                ))
            }
        };
        crate::core::vdf::verify_and_consume_vdf(vdf_c, vdf_p, Some(&body.username)).await?;

        if let Some(ct) = &body.pqc_ciphertext {
            let _pqc_shared_secret = crate::core::pqc::verify_and_decapsulate_pqc(ct).await?;
        }
    }

    match auth::login(&state, &body.username, &body.password).await {
        Ok(res) => {
            // Successful login: reset failure counter for this account
            let mut buckets = state.rate_limits.lock().await;
            buckets.remove(&fail_key);
            Ok(Json(res))
        }
        Err(err) => {
            // Failed login: increment failure counter
            let _ = crate::core::security::rate_limit_hit(&state, &fail_key, 100, 30_000).await;
            Err(err)
        }
    }
}

async fn auth_recover_handler(
    State(state): State<SharedState>,
    Json(body): Json<AuthRecoverRequest>,
) -> ApiResult<impl IntoResponse> {
    let clean_user = crate::core::security::normalize_username(&body.username);
    let rate_key = format!("recover:user:{}", clean_user);
    if crate::core::security::rate_limit_hit(&state, rate_key, 3, 30_000).await {
        return Err(ApiError::too_many_requests(
            "Too many recovery attempts for this account. Please wait 30 seconds.",
        ));
    }

    if let Some(token) = &body.cap_token {
        crate::core::cap::verify_and_consume_cap_token(token, "recover").await?;
    }

    auth::recover(
        &state,
        &body.username,
        &body.recovery_words,
        &body.new_password,
    )
    .await
    .map(Json)
}

async fn auth_me_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    let token = bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing session."))?;
    auth::me(&state, &token).await.map(Json)
}

async fn auth_logout_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    let token = bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing session."))?;
    auth::logout(&state, &token).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn auth_delete_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<AuthDeleteRequest>,
) -> ApiResult<impl IntoResponse> {
    let token = bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("Missing session."))?;
    auth::delete_account(&state, &token, &body.password).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn auth_username_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<UsernameRequest>,
) -> ApiResult<impl IntoResponse> {
    let user = authenticated_user(&state, &headers).await?;
    let rate_key = format!("username_change:user:{}", user.id);
    if crate::core::security::rate_limit_hit(&state, rate_key, 3, 10 * 60 * 1000).await {
        return Err(ApiError::too_many_requests(
            "Too many username change requests. Please wait a few minutes.",
        ));
    }

    auth::change_username(&state, &user, &body.username)
        .await
        .map(Json)
}

async fn profile_image_upload_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    let user = authenticated_user(&state, &headers).await?;
    let rate_key = format!("profile_image:user:{}", user.id);
    if crate::core::security::rate_limit_hit(&state, rate_key, 5, 60_000).await {
        return Err(ApiError::too_many_requests(
            "Too many profile image uploads. Please wait a minute.",
        ));
    }
    user::upload_profile_image(&state, &user, multipart)
        .await
        .map(Json)
}

async fn room_icon_upload_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(room_id): AxumPath<String>,
    multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    let user = authenticated_user(&state, &headers).await?;
    let rate_key = format!("room_icon:user:{}", user.id);
    if crate::core::security::rate_limit_hit(&state, rate_key, 5, 60_000).await {
        return Err(ApiError::too_many_requests(
            "Too many room icon uploads. Please wait a minute.",
        ));
    }
    room::upload_room_icon(&state, &user, &room_id, multipart)
        .await
        .map(Json)
}

async fn admin_overview_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::admin_overview(&state, &admin).await.map(Json)
}

async fn admin_features_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<FeatureRequest>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::set_feature(&state, &admin, &body.key, body.enabled)
        .await
        .map(Json)
}

async fn admin_user_disabled_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<DisabledRequest>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::set_user_disabled(&state, &admin, &user_id, body.disabled)
        .await
        .map(Json)
}

async fn admin_user_banned_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<BannedRequest>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::set_user_banned(&state, &admin, &user_id, body.banned)
        .await
        .map(Json)
}

async fn admin_user_delete_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::delete_user_account(&state, &admin, &user_id)
        .await
        .map(Json)
}

async fn admin_user_badges_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    AxumPath(user_id): AxumPath<String>,
    Json(body): Json<BadgesRequest>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::set_user_badges(&state, &admin, &user_id, &body.badges)
        .await
        .map(Json)
}

async fn admin_users_purge_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<PurgeUsersRequest>,
) -> ApiResult<impl IntoResponse> {
    let admin = authenticated_user(&state, &headers).await?;
    admin::purge_accounts(
        &state,
        &admin,
        body.created_after_ms,
        body.created_before_ms,
        body.min_username_len,
        body.max_username_len,
        body.username_contains.as_deref(),
    )
    .await
    .map(Json)
}

struct CachedRelease {
    expires_at: std::time::Instant,
    status: StatusCode,
    body: axum::body::Bytes,
}

static RELEASE_CACHE: tokio::sync::Mutex<Option<CachedRelease>> = tokio::sync::Mutex::const_new(None);
static RELEASE_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

async fn latest_release_handler() -> Response {
    let now = std::time::Instant::now();
    {
        let cache = RELEASE_CACHE.lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.expires_at > now {
                return Response::builder()
                    .status(cached.status)
                    .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                    .body(axum::body::Body::from(cached.body.clone()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
        }
    }

    let client = RELEASE_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("QxProtocol-ReleaseProxy/0.1 (+https://github.com/lqxp)")
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .expect("failed to build reqwest client")
    });

    let response = match client
        .get("https://api.github.com/repos/lqxp/app/releases/latest")
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return ApiError::new(StatusCode::BAD_GATEWAY, "Failed to fetch release.").into_response(),
    };

    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(_) => return ApiError::new(StatusCode::BAD_GATEWAY, "Failed to read release response.").into_response(),
    };

    if status.is_success() {
        let mut cache = RELEASE_CACHE.lock().await;
        *cache = Some(CachedRelease {
            expires_at: now + std::time::Duration::from_secs(15 * 60),
            status,
            body: body.clone(),
        });
    }

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn webchat_page(State(state): State<SharedState>, headers: HeaderMap) -> impl IntoResponse {
    let path =
        PathBuf::from(&state.config.network.public_dir).join(&state.config.network.webchat_index);
    let origin = public_origin(&headers, &state.config.api.public_domain);
    serve_webchat_index(&path, origin.as_deref(), &state).await
}

async fn public_asset_handler(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let Some(full_path) = safe_child_path(&state.config.network.public_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve_file(&full_path).await
}

async fn app_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let Some(full_path) = safe_child_path(&state.config.network.public_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if !full_path.exists() {
        let index = PathBuf::from(&state.config.network.public_dir)
            .join(&state.config.network.webchat_index);
        return serve_webchat_index(&index, None, &state).await;
    }

    serve_file(&full_path).await
}

async fn upload_asset(
    State(state): State<SharedState>,
    AxumPath(path): AxumPath<String>,
) -> impl IntoResponse {
    let Some(full_path) = safe_child_path(&state.config.network.upload_dir, &path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !messaging::upload_is_live(&state, &path).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_file(&full_path).await
}

fn safe_child_path(base: &str, raw_path: &str) -> Option<PathBuf> {
    let relative = Path::new(raw_path.trim_start_matches('/'));
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(Path::new(base).join(relative))
}

async fn serve_file(path: &Path) -> Response {
    match fs::read(path).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime.as_ref())
                .header("x-content-type-options", "nosniff")
                .header("content-security-policy", "sandbox; default-src 'none'")
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_webchat_index(path: &Path, origin: Option<&str>, state: &SharedState) -> Response {
    match fs::read_to_string(path).await {
        Ok(mut html) => {
            let runtime = runtime_config_payload(origin, state);
            let bootstrap = format!("<script>window.__QXP_RUNTIME__ = {};</script>", runtime);
            if html.contains("</head>") {
                html = html.replacen("</head>", &format!("{bootstrap}</head>"), 1);
            } else {
                html = format!("{bootstrap}{html}");
            }
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html; charset=utf-8")
                .body(axum::body::Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn runtime_config_payload(origin: Option<&str>, state: &SharedState) -> serde_json::Value {
    let public_domain = state.config.api.public_domain.trim();
    let ws_origin = origin
        .map(str::to_owned)
        .or_else(|| {
            if public_domain.is_empty() {
                None
            } else {
                Some(format!("https://{public_domain}"))
            }
        })
        .unwrap_or_default();

    let ws_url = if ws_origin.is_empty() {
        String::new()
    } else if ws_origin.starts_with("https://") {
        format!("wss://{}/ws", ws_origin.trim_start_matches("https://"))
    } else if ws_origin.starts_with("http://") {
        format!("ws://{}/ws", ws_origin.trim_start_matches("http://"))
    } else {
        format!("wss://{ws_origin}/ws")
    };

    let rtc = &state.config.rtc;
    let calls_enabled = true;
    let calls_unavailable_reason = String::new();

    let servers: Vec<serde_json::Value> = rtc
        .resolved_servers()
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "label": s.label,
                "hint": s.hint,
                "urls": s.urls,
                "username": s.username,
                "credential": s.credential
            })
        })
        .collect();

    let default_server = if !rtc.default_turn_server.is_empty() {
        rtc.default_turn_server.clone()
    } else if let Some(first) = rtc.resolved_servers().first() {
        first.id.clone()
    } else {
        String::new()
    };

    let mut payload = json!({
        "rtc": {
            "relayOnly": rtc.relay_only,
            "servers": servers,
            "defaultTurnServer": default_server,
            "turnUrls": rtc.turn_urls,
            "turnUsername": rtc.turn_username,
            "turnCredential": rtc.turn_credential,
            "callsEnabled": calls_enabled,
            "callsUnavailableReason": calls_unavailable_reason
        },
        "api": {
            "origin": origin.unwrap_or_default(),
            "publicDomain": public_domain,
            "wsUrl": ws_url
        }
    });

    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "latestVersion".to_owned(),
            json!(state
                .config
                .network
                .latest_version
                .as_deref()
                .unwrap_or("")
                .trim()),
        );
    }

    payload
}

fn public_origin(headers: &HeaderMap, configured_domain: &str) -> Option<String> {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(host) = host {
        let forwarded_proto = headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let proto = match forwarded_proto {
            Some(p) => p,
            None => {
                let host_part = host.split(':').next().unwrap_or("");
                let is_local = host_part.eq_ignore_ascii_case("localhost")
                    || host_part == "127.0.0.1"
                    || host_part == "[::1]";
                let is_private = host_part
                    .parse::<IpAddr>()
                    .map(|ip| match ip {
                        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
                        IpAddr::V6(v6) => v6.is_loopback(),
                    })
                    .unwrap_or(false);
                if is_local || is_private { "http" } else { "https" }
            }
        };
        return Some(format!("{proto}://{host}"));
    }

    let configured = configured_domain.trim();
    if configured.is_empty() {
        return None;
    }
    Some(format!("https://{configured}"))
}

async fn authenticated_user(
    state: &SharedState,
    headers: &HeaderMap,
) -> ApiResult<AuthenticatedUser> {
    let token = bearer_token(headers).ok_or_else(|| ApiError::unauthorized("Missing session."))?;
    state
        .accounts
        .authenticate_token(&token)
        .await?
        .ok_or_else(|| ApiError::unauthorized("Invalid session."))
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

async fn ws_upgrade_handler(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}
