use std::{collections::HashMap, time::{Duration, Instant}};

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::core::presence::SharedState;

static DOWNLOAD_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
static DOWNLOAD_CACHE: tokio::sync::Mutex<Option<CachedDownload>> =
    tokio::sync::Mutex::const_new(None);

const CACHE_TTL_SUCCESS: Duration = Duration::from_secs(10 * 60);
const CACHE_TTL_ERROR: Duration = Duration::from_secs(90);
const FETCH_PER_PAGE: u8 = 20;

fn app_repo() -> String {
    let configured = std::env::var("LQXP_APP_REPO")
        .or_else(|_| std::env::var("APP_REPO"))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    configured.unwrap_or_else(|| "lqxp/app".to_owned())
}

fn download_client() -> &'static reqwest::Client {
    DOWNLOAD_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("QxProtocol-DownloadProxy/0.1 (+https://github.com/lqxp)")
            .timeout(Duration::from_secs(8))
            .build()
            .expect("failed to build download client")
    })
}

fn github_auth_header(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim().to_owned();
        if !token.is_empty() {
            return req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
    }
    req
}

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    #[serde(default)]
    pub limit: Option<u8>,
}

fn clamp_limit(raw: Option<u8>) -> usize {
    match raw {
        None => 10,
        Some(0) => 10,
        Some(n) => (n.min(30) as usize).max(1),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BinaryInfo {
    name: String,
    size: u64,
    downloads: u64,
    url: String,
    content_type: String,
    sha256: String,
    digest: String,
    platform: String,
    arch: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChecksumInfo {
    name: String,
    sha256: String,
    url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitInfo {
    sha: String,
    short_sha: String,
    message: String,
    author: String,
    date: String,
    url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseSummary {
    tag: String,
    version: String,
    name: String,
    body: String,
    published_at: String,
    url: String,
    prerelease: bool,
    draft: bool,
    binaries: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDetail {
    tag: String,
    version: String,
    name: String,
    body: String,
    published_at: String,
    url: String,
    prerelease: bool,
    draft: bool,
    target: String,
}

struct CachedDownload {
    expires_at: Instant,
    status: StatusCode,
    body: axum::body::Bytes,
    // Full (un-sliced) payload used to serve smaller `?limit=` from cache.
    full: Option<serde_json::Value>,
}

fn strip_version(tag: &str) -> String {
    tag.trim().trim_start_matches(['v', 'V']).to_owned()
}

fn detect_platform(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".apk") || lower.contains("android") {
        "android".to_owned()
    } else if lower.ends_with(".ipa") {
        "ios".to_owned()
    } else if lower.ends_with(".msi") || lower.ends_with(".exe") || lower.contains("windows") {
        "windows".to_owned()
    } else if lower.ends_with(".dmg") || lower.contains("macos") || lower.contains("darwin") {
        "macos".to_owned()
    } else if lower.ends_with(".appimage")
        || lower.ends_with(".deb")
        || lower.ends_with(".rpm")
        || lower.contains("linux")
        || lower.ends_with(".tar.gz")
    {
        "linux".to_owned()
    } else {
        "other".to_owned()
    }
}

fn detect_arch(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("aarch64") || lower.contains("arm64") {
        "arm64".to_owned()
    } else if lower.contains("x86_64") || lower.contains("x64") || lower.contains("amd64") {
        "x64".to_owned()
    } else if lower.contains("armv7") || lower.contains("armhf") {
        "armv7".to_owned()
    } else if lower.contains("universal") {
        "universal".to_owned()
    } else {
        String::new()
    }
}

/// Les signatures / métadonnées ne sont pas des binaires installables :
/// on les expose via `checksums` mais on les retire de `binaries`.
fn is_binary_asset(name: &str, content_type: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".sig")
        || lower.ends_with(".asc")
        || lower.ends_with(".pem")
        || lower.ends_with(".pub")
        || lower == "latest.json"
        || lower.ends_with("latest.json")
    {
        return false;
    }
    if lower.contains("checksum") || lower.contains("sha256") || lower.contains("sha512") {
        return false;
    }
    if content_type == "application/pgp-signature" {
        return false;
    }
    true
}

fn sha256_of_digest(digest: &str) -> String {
    digest
        .strip_prefix("sha256:")
        .unwrap_or(digest)
        .trim()
        .to_owned()
}

fn asset_to_binary(v: &serde_json::Value) -> Option<(BinaryInfo, ChecksumInfo)> {
    let name = v.get("name")?.as_str()?.to_owned();
    let url = v
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_owned();
    if name.is_empty() || url.is_empty() {
        return None;
    }
    let size = v.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
    let downloads = v
        .get("download_count")
        .and_then(|d| d.as_u64())
        .unwrap_or(0);
    let content_type = v
        .get("content_type")
        .and_then(|c| c.as_str())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let digest = v
        .get("digest")
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_owned();
    let sha256 = sha256_of_digest(&digest);
    let binary = BinaryInfo {
        platform: detect_platform(&name),
        arch: detect_arch(&name),
        name: name.clone(),
        size,
        downloads,
        url: url.clone(),
        content_type,
        sha256: sha256.clone(),
        digest,
    };
    let checksum = ChecksumInfo {
        name,
        sha256,
        url,
    };
    Some((binary, checksum))
}

fn parse_release_detail(v: &serde_json::Value) -> Option<(ReleaseDetail, Vec<BinaryInfo>, Vec<ChecksumInfo>)> {
    let tag = v.get("tag_name")?.as_str()?.to_owned();
    let mut binaries = Vec::new();
    let mut checksums = Vec::new();
    if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
        for a in assets {
            if let Some((bin, sum)) = asset_to_binary(a) {
                checksums.push(sum.clone());
                if is_binary_asset(
                    &bin.name,
                    &bin.content_type,
                ) {
                    binaries.push(bin);
                }
            }
        }
    }
    // Tri stable pour la vitrine : plus gros téléchargements d'abord.
    binaries.sort_by(|a, b| b.downloads.cmp(&a.downloads).then(a.name.cmp(&b.name)));
    let detail = ReleaseDetail {
        version: strip_version(&tag),
        tag,
        name: v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_owned(),
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_owned(),
        published_at: v
            .get("published_at")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_owned(),
        url: v
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_owned(),
        prerelease: v.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
        draft: v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        target: v
            .get("target_commitish")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_owned(),
    };
    Some((detail, binaries, checksums))
}

fn parse_release_summary(v: &serde_json::Value) -> Option<ReleaseSummary> {
    let tag = v.get("tag_name")?.as_str()?.to_owned();
    let binaries = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|assets| {
            assets
                .iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?;
                    let ct = a.get("content_type").and_then(|c| c.as_str()).unwrap_or("");
                    Some(is_binary_asset(name, ct))
                })
                .filter(|b| *b)
                .count()
        })
        .unwrap_or(0);
    Some(ReleaseSummary {
        version: strip_version(&tag),
        tag,
        name: v
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_owned(),
        body: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_owned(),
        published_at: v
            .get("published_at")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_owned(),
        url: v
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_owned(),
        prerelease: v.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
        draft: v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
        binaries,
    })
}

fn parse_commit(v: &serde_json::Value) -> Option<CommitInfo> {
    let sha = v.get("sha")?.as_str()?.to_owned();
    let commit = v.get("commit")?;
    let message = commit
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_owned();
    let author = commit
        .get("author")
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| {
            v.get("author")
                .and_then(|a| a.get("login"))
                .and_then(|l| l.as_str())
        })
        .unwrap_or_default()
        .to_owned();
    let date = commit
        .get("author")
        .and_then(|a| a.get("date"))
        .and_then(|d| d.as_str())
        .or_else(|| {
            commit
                .get("committer")
                .and_then(|c| c.get("date"))
                .and_then(|d| d.as_str())
        })
        .unwrap_or_default()
        .to_owned();
    let url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_owned();
    Some(CommitInfo {
        short_sha: sha.chars().take(7).collect(),
        sha,
        message,
        author,
        date,
        url,
    })
}

async fn fetch_json(url: &str) -> Result<serde_json::Value, String> {
    let client = download_client();
    let res = github_auth_header(
        client
            .get(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28"),
    )
    .send()
    .await
    .map_err(|e| format!("github request failed: {e}"))?;
    let status = res.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("github rate limit exceeded".to_owned());
    }
    if !status.is_success() {
        return Err(format!("github responded {status}"));
    }
    let bytes = res
        .bytes()
        .await
        .map_err(|e| format!("invalid github payload: {e}"))?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("invalid github payload: {e}"))
}

fn slice_json_array(full: &serde_json::Value, limit: usize) -> serde_json::Value {
    match full {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().take(limit).cloned().collect())
        }
        other => other.clone(),
    }
}

fn build_response(full: &serde_json::Value, limit: usize) -> serde_json::Value {
    let mut out = full.clone();
    if let Some(obj) = out.as_object_mut() {
        if let Some(commits) = obj.get("commits").cloned() {
            obj.insert("commits".to_owned(), slice_json_array(&commits, limit));
        }
        if let Some(releases) = obj.get("releases").cloned() {
            obj.insert("releases".to_owned(), slice_json_array(&releases, limit));
            // Alias attendu par certaines vitrines.
            obj.insert("history".to_owned(), slice_json_array(&releases, limit));
            obj.insert(
                "versionHistory".to_owned(),
                slice_json_array(&releases, limit),
            );
        }
    }
    out
}

fn corsify(mut res: Response) -> Response {
    let headers = res.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::HeaderValue::from_static("GET, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::HeaderValue::from_static("Content-Type"),
    );
    headers.insert(header::VARY, header::HeaderValue::from_static("Origin"));
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=600"),
    );
    res
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    corsify(
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(axum::body::Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
    )
}

/// `GET /api/download` — agrégat public pour le site vitrine.
///
/// Réponse (JSON) :
/// ```json
/// {
///   "tag": "v1.20.6", "version": "1.20.6",
///   "release": { "tag", "name", "body", "publishedAt", "url", ... },
///   "binaries": [ { "name", "size", "downloads", "url", "sha256", "platform", "arch" } ],
///   "checksum": { "<fichier>": "<sha256>" },
///   "checksums": [ { "name", "sha256", "url" } ],
///   "commits": [ { "sha", "message", "author", "date", "url" } ],
///   "releases": [ { "tag", "version", "name", "publishedAt", "url" } ]
/// }
/// ```
/// `releases` est l'historique des versions (array), `binaries` les binaires
/// de la dernière release, `checksum` la map fichier -> sha256 (digest GitHub).
pub async fn download_handler(
    State(state): State<SharedState>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    if crate::core::security::rate_limit_hit(&state, "download:global".to_string(), 30, 60_000).await {
        return json_response(
            StatusCode::TOO_MANY_REQUESTS,
            &serde_json::json!({ "error": "Rate limit exceeded." }),
        );
    }
    let limit = clamp_limit(query.limit);

    // Cache : on stocke le payload complet (per_page=20) puis on découpe
    // selon `?limit=` pour chaque réponse.
    let cached_hit: Option<(StatusCode, axum::body::Bytes, Option<serde_json::Value>)> = {
        let cache = DOWNLOAD_CACHE.lock().await;
        match cache.as_ref() {
            Some(cached) if cached.expires_at > Instant::now() => Some((
                cached.status,
                cached.body.clone(),
                cached.full.clone(),
            )),
            _ => None,
        }
    };
    if let Some((status, body, full)) = cached_hit {
        if let Some(full) = full {
            return json_response(status, &build_response(&full, limit));
        }
        return corsify(
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(axum::body::Body::from(body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        );
    }

    let repo = app_repo();
    let base = format!("https://api.github.com/repos/{repo}");
    let latest_url = format!("{base}/releases/latest");
    let releases_url = format!("{base}/releases?per_page={FETCH_PER_PAGE}");
    let commits_url = format!("{base}/commits?per_page={FETCH_PER_PAGE}");

    let (latest, history, commits) = tokio::join!(
        fetch_json(&latest_url),
        fetch_json(&releases_url),
        fetch_json(&commits_url),
    );

    let latest_value = match latest {
        Ok(v) => v,
        Err(e) => {
            let payload = serde_json::json!({ "error": "Failed to fetch release.", "detail": e });
            let mut cache = DOWNLOAD_CACHE.lock().await;
            *cache = Some(CachedDownload {
                expires_at: Instant::now() + CACHE_TTL_ERROR,
                status: StatusCode::BAD_GATEWAY,
                body: axum::body::Bytes::from(serde_json::to_vec(&payload).unwrap_or_default()),
                full: None,
            });
            return json_response(StatusCode::BAD_GATEWAY, &payload);
        }
    };

    let (release, binaries, checksums) = match parse_release_detail(&latest_value) {
        Some(t) => t,
        None => {
            let payload = serde_json::json!({ "error": "Invalid release payload." });
            return json_response(StatusCode::BAD_GATEWAY, &payload);
        }
    };

    let history_list: Vec<ReleaseSummary> = match history {
        Ok(v) => v
            .as_array()
            .map(|arr| arr.iter().filter_map(parse_release_summary).collect())
            .unwrap_or_default(),
        Err(e) => {
            tracing::debug!("download: releases history failed: {e}");
            Vec::new()
        }
    };
    let commit_list: Vec<CommitInfo> = match commits {
        Ok(v) => v
            .as_array()
            .map(|arr| arr.iter().filter_map(parse_commit).collect())
            .unwrap_or_default(),
        Err(e) => {
            tracing::debug!("download: commits failed: {e}");
            Vec::new()
        }
    };

    let checksum_map: HashMap<String, String> = checksums
        .iter()
        .map(|c| (c.name.clone(), c.sha256.clone()))
        .collect();

    let fetched_at = crate::core::models::now_ms();
    let full = serde_json::json!({
        "tag": release.tag,
        "version": release.version,
        "release": release,
        "latestRelease": release,
        "binaries": binaries,
        "latestBinaries": binaries,
        "checksum": checksum_map,
        "checksums": checksums,
        "commits": commit_list,
        "releases": history_list,
        "history": history_list,
        "versionHistory": history_list,
        "repo": repo,
        "fetchedAt": fetched_at,
    });

    let payload = build_response(&full, limit);
    {
        let mut cache = DOWNLOAD_CACHE.lock().await;
        *cache = Some(CachedDownload {
            expires_at: Instant::now() + CACHE_TTL_SUCCESS,
            status: StatusCode::OK,
            body: axum::body::Bytes::new(),
            full: Some(full),
        });
    }
    // On borne le rate-limit GitHub : on tolère un historique vide plutôt
    // qu'une 502 dès que les endpoints secondaires sont limités.
    if history_list.is_empty() && commit_list.is_empty() {
        tracing::debug!("download: serving latest only (history+commits unavailable)");
    }
    json_response(StatusCode::OK, &payload)
}

pub async fn download_options_handler() -> Response {
    corsify(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_stripped() {
        assert_eq!(strip_version("v1.20.6"), "1.20.6");
        assert_eq!(strip_version("V2.0"), "2.0");
        assert_eq!(strip_version("1.0.0"), "1.0.0");
    }

    #[test]
    fn platforms_are_detected() {
        assert_eq!(detect_platform("QxChat_1.20.6_x64_en-US.msi"), "windows");
        assert_eq!(detect_platform("QxChat_1.20.6_x64.dmg"), "macos");
        assert_eq!(detect_platform("QxChat_1.20.6_amd64.deb"), "linux");
        assert_eq!(detect_platform("QxChat_1.20.6_aarch64.apk"), "android");
        assert_eq!(detect_platform("QxChat_1.20.6_unsigned.ipa"), "ios");
        assert_eq!(detect_platform("QxChat_1.20.6_arm64.deb.sig"), "other");
    }

    #[test]
    fn sig_files_are_not_binaries() {
        assert!(!is_binary_asset("QxChat_1.20.6_amd64.deb.sig", "application/pgp-signature"));
        assert!(!is_binary_asset("latest.json", "application/json"));
        assert!(is_binary_asset("QxChat_1.20.6_amd64.deb", "application/x-debian-package"));
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), 10);
        assert_eq!(clamp_limit(Some(0)), 10);
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(200)), 30);
    }
}
