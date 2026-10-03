//! Process-detectable game list served to desktop clients.
//!
//! Upstream is Discord's official detectable-applications feed (same schema
//! as the arrpc mirror: names, executables, artwork hashes…). The server
//! fetches it, compacts it to a minified `exe -> display name` map and serves
//! that at `GET /api/activity/detectable`. Desktop clients cache it for days
//! and hand it to their local Tauri `activity` plugin — no client ever
//! fetches the feed directly, and the built-in plugin table stays as the
//! offline fallback.
//!
//! Refresh: lazy with a 2 h backend TTL. A stale cache is served immediately
//! while a background task re-fetches; a failed fetch keeps serving the last
//! good payload.

use std::collections::HashMap;
use std::time::Duration;

use crate::core::models::now_ms;

/// Official Discord detectable-applications feed (no auth required).
pub const DETECTABLE_URL: &str = "https://discord.com/api/v9/applications/detectable";
/// Upstream re-fetch cadence (clients cache far longer).
pub const DETECTABLE_TTL_MS: u64 = 2 * 3600 * 1000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Raw download sanity cap (upstream is ~13 MB).
const MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;
/// Compacted map alloc guard against hostile payloads.
const MAX_ENTRIES: usize = 200_000;
/// Display-name length cap (mirrors the profile activity cap).
const MAX_NAME_CHARS: usize = 96;

#[derive(Debug, Clone, Default)]
pub struct DetectableSnapshot {
    pub updated_at: u64,
    /// Minified `{"v":1,"updatedAt":…,"count":…,"games":{exe:name}}`, served as-is.
    pub payload: String,
}

static CACHE: std::sync::OnceLock<tokio::sync::RwLock<Option<DetectableSnapshot>>> =
    std::sync::OnceLock::new();

fn cache() -> &'static tokio::sync::RwLock<Option<DetectableSnapshot>> {
    CACHE.get_or_init(|| tokio::sync::RwLock::new(None))
}

fn empty_payload() -> String {
    serde_json::json!({"v": 1, "updatedAt": 0, "count": 0, "games": {}}).to_string()
}

/// Lowercased executable stem: basename after `/` or `\`, `.exe` stripped.
pub fn exe_key(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let lower = base.to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// Pure compaction: feed entries (`[{name, executables:[{name, is_launcher}]}]`)
/// into `exe -> display name`. First entry wins;
/// launchers are skipped (they would shadow the real game process).
pub fn compact_detectable(raw: &serde_json::Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let list = match raw {
        serde_json::Value::Array(items) => items.as_slice(),
        serde_json::Value::Object(_) => raw
            .get("detectable")
            .and_then(|v| v.as_array())
            .map(|v| v.as_slice())
            .unwrap_or(&[]),
        _ => &[],
    };
    for entry in list {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let display: String = name.chars().take(MAX_NAME_CHARS).collect();
        let empty: Vec<serde_json::Value> = Vec::new();
        let exes = entry
            .get("executables")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        for exe in exes {
            if exe
                .get("is_launcher")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let raw_name = exe.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let key = exe_key(raw_name);
            if key.is_empty() || key == "exe" {
                continue;
            }
            out.entry(key).or_insert_with(|| display.clone());
            if out.len() >= MAX_ENTRIES {
                return out;
            }
        }
    }
    out
}

fn build_payload(map: &HashMap<String, String>, updated_at: u64) -> String {
    serde_json::json!({
        "v": 1,
        "updatedAt": updated_at,
        "count": map.len(),
        "games": map,
    })
    .to_string()
}

async fn fetch_upstream() -> Option<HashMap<String, String>> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent("lqxp-activity/1")
        .build()
        .ok()?;
    let res = client.get(DETECTABLE_URL).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let bytes = res.bytes().await.ok()?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return None;
    }
    let raw: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(compact_detectable(&raw))
}

async fn refresh() {
    let Some(map) = fetch_upstream().await else {
        return;
    };
    let now = now_ms();
    let payload = build_payload(&map, now);
    *cache().write().await = Some(DetectableSnapshot { updated_at: now, payload });
}

/// Fetched artwork, served to clients so no browser ever hotlinks Discord or
/// third-party CDNs directly (IP leak + CSP).
#[derive(Debug, Clone)]
pub struct ActivityAsset {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

const ASSET_TTL_MS: u64 = 24 * 3600 * 1000;
const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
const MAX_ASSET_ENTRIES: usize = 256;
const ASSET_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
struct AssetCacheEntry {
    fetched_at: u64,
    asset: Option<ActivityAsset>,
}

static ASSET_CACHE: std::sync::OnceLock<tokio::sync::Mutex<HashMap<String, AssetCacheEntry>>> =
    std::sync::OnceLock::new();

fn asset_cache(
) -> &'static tokio::sync::Mutex<HashMap<String, AssetCacheEntry>> {
    ASSET_CACHE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// Proxies one artwork URL (already resolved to full `https://` by the
/// client: Discord CDN, `media.discordapp.net`, `i.scdn.co`, or any direct
/// image URL). Same SSRF discipline as link previews: DNS-pinned fetch,
/// public IPs only, `image/*` content, 2 MiB cap, 24 h cache.
pub async fn fetch_activity_asset(raw_url: &str) -> Option<ActivityAsset> {
    let (parsed, pinned_addr) = crate::linkpreview::validate_and_pin_url(raw_url)?;
    let cache_key = parsed.to_string();
    let now = now_ms();
    if let Some(hit) = asset_cache().lock().await.get(&cache_key) {
        if now.saturating_sub(hit.fetched_at) < ASSET_TTL_MS {
            return hit.asset.clone();
        }
    }
    let host = parsed.host_str()?.to_owned();
    let client = reqwest::Client::builder()
        .timeout(ASSET_FETCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(3))
        .resolve(&host, pinned_addr)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                return attempt.stop();
            }
            let same_host = attempt
                .previous()
                .last()
                .and_then(|u| u.host_str().map(str::to_owned))
                .zip(attempt.url().host_str().map(str::to_owned))
                .map(|(a, b)| a.eq_ignore_ascii_case(&b))
                .unwrap_or(false);
            if !same_host {
                return attempt.stop();
            }
            match crate::linkpreview::validate_and_pin_url(attempt.url().as_str()) {
                Some(_) => attempt.follow(),
                None => attempt.stop(),
            }
        }))
        .user_agent("lqxp-activity/1")
        .build()
        .ok()?;
    let result = async {
        let mut resp = client.get(parsed.as_str()).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !content_type.starts_with("image/") {
            return None;
        }
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_ASSET_BYTES * 2 {
                return None;
            }
        }
        let final_url = resp.url().clone();
        crate::linkpreview::validate_and_pin_url(final_url.as_str())?;
        let mut bytes_read = 0usize;
        let mut buf: Vec<u8> = Vec::with_capacity(32 * 1024);
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    bytes_read += chunk.len();
                    if bytes_read > MAX_ASSET_BYTES {
                        return None;
                    }
                    buf.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return None,
            }
        }
        if buf.is_empty() {
            return None;
        }
        Some(ActivityAsset { content_type, bytes: buf })
    }
    .await;
    {
        let mut cache = asset_cache().lock().await;
        cache.retain(|_, e| now.saturating_sub(e.fetched_at) < ASSET_TTL_MS);
        while cache.len() >= MAX_ASSET_ENTRIES {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, e)| e.fetched_at)
                .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest);
            } else {
                break;
            }
        }
        cache.insert(cache_key, AssetCacheEntry { fetched_at: now, asset: result.clone() });
    }
    result
}

/// Minified detectable payload, refreshed lazily in the background once
/// stale. Never fails: worst case the empty map (clients fall back to their
/// built-in table).
pub async fn detectable_payload() -> String {
    let now = now_ms();
    if let Some(snap) = cache().read().await.clone() {
        if now.saturating_sub(snap.updated_at) < DETECTABLE_TTL_MS {
            return snap.payload;
        }
        tokio::spawn(async move {
            refresh().await;
        });
        return snap.payload;
    }
    refresh().await;
    cache()
        .read()
        .await
        .clone()
        .map(|snap| snap.payload)
        .unwrap_or_else(empty_payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exe_key_normalizes_stems() {
        assert_eq!(exe_key("Overwatch.exe"), "overwatch");
        assert_eq!(exe_key("_ptr_/WoW.EXE"), "wow");
        assert_eq!(exe_key("C:\\Games\\BG3_DX11.exe"), "bg3_dx11");
        assert_eq!(exe_key("firefox"), "firefox");
        assert_eq!(exe_key(""), "");
    }

    #[test]
    fn compact_skips_launchers_dedupes_and_truncates() {
        let raw = json!([
            {
                "name": "Overwatch",
                "executables": [
                    {"is_launcher": false, "name": "overwatch.exe", "os": "win32"},
                    {"is_launcher": true, "name": "Battle.net.exe", "os": "win32"}
                ]
            },
            {
                "name": "World of Warcraft",
                "executables": [
                    {"is_launcher": false, "name": "_ptr_/wow.exe", "os": "win32"},
                    {"is_launcher": false, "name": "overwatch.exe", "os": "win32"}
                ]
            },
            {"name": "   ", "executables": [{"name": "x.exe"}]},
            {"name": "Broken", "executables": "nope"},
            "garbage"
        ]);
        let map = compact_detectable(&raw);
        // First entry wins the shared exe; the launcher never lands.
        assert_eq!(map.get("overwatch"), Some(&"Overwatch".to_string()));
        assert_eq!(map.get("wow"), Some(&"World of Warcraft".to_string()));
        assert!(!map.contains_key("battle.net"));
        assert!(!map.contains_key("x"));
    }

    #[test]
    fn asset_proxy_rejects_non_public_targets() {
        // IP literals need no DNS: loopback, private, link-local refused.
        for bad in [
            "http://127.0.0.1/x.png",
            "https://10.0.0.5/x.png",
            "https://192.168.1.1/x.png",
            "https://169.254.169.254/x.png",
            "http://[::1]/x.png",
            "ftp://example.com/x.png",
            "not a url",
            "",
        ] {
            assert!(
                crate::linkpreview::validate_and_pin_url(bad).is_none(),
                "must reject {bad}"
            );
        }
    }

    #[test]
    fn compact_rejects_garbage_shapes() {
        assert!(compact_detectable(&json!(null)).is_empty());
        assert!(compact_detectable(&json!({"detectable": "nope"})).is_empty());
        assert!(compact_detectable(&json!({"detectable": [{"name": "G", "executables": [{"name": "g.exe"}]}]}))
            .contains_key("g"));
    }
}
