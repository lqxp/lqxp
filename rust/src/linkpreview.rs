use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use regex::Regex;
use reqwest::{redirect, Client};
use tokio::sync::Mutex;
use tracing::debug;
use url::Url;

use crate::models::LinkPreview;

const MAX_HTML_BYTES: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(4);
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Conservative URL matcher — matches http(s)://host[/path...] up to whitespace.
        Regex::new(r#"https?://[^\s<>"'`\\]+"#).unwrap()
    })
}

fn og_meta_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Match <meta ... property|name="...og:..." content="..."/> in any attribute order.
        Regex::new(r#"(?is)<meta\b[^>]*>"#).unwrap()
    })
}

fn title_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).unwrap())
}

pub fn find_first_url(text: &str) -> Option<String> {
    url_regex().find(text).map(|m| m.as_str().to_owned())
}

fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: &Ipv4Addr) -> bool {
    if ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
    {
        return false;
    }
    let o = ip.octets();
    // private ranges
    if o[0] == 10 { return false; }
    if o[0] == 172 && (16..=31).contains(&o[1]) { return false; }
    if o[0] == 192 && o[1] == 168 { return false; }
    // shared address space 100.64.0.0/10
    if o[0] == 100 && (64..=127).contains(&o[1]) { return false; }
    // benchmarking 198.18.0.0/15
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) { return false; }
    // 169.254.x (link-local handled above) but also 0.x is unspecified
    true
}

fn is_public_ipv6(ip: &Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    let seg = ip.segments();
    // ULA fc00::/7
    if (seg[0] & 0xfe00) == 0xfc00 { return false; }
    // Link-local fe80::/10
    if (seg[0] & 0xffc0) == 0xfe80 { return false; }
    // IPv4-mapped ::ffff:0:0/96 → check the embedded v4
    if seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0xffff {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        return is_public_ipv4(&v4);
    }
    true
}

fn validate_url(raw: &str) -> Option<Url> {
    let parsed = Url::parse(raw).ok()?;
    match parsed.scheme() {
        "http" | "https" => (),
        _ => return None,
    }
    let host = parsed.host_str()?;
    if host.is_empty() { return None; }
    if host.eq_ignore_ascii_case("localhost") { return None; }
    if host.len() > 253 { return None; }

    // Resolve all addresses and require every one to be public. If DNS
    // resolves to a mix of public/private, reject conservatively.
    let port = parsed.port_or_known_default().unwrap_or(0);
    let target = format!("{}:{}", host, port);
    let addrs = target.to_socket_addrs().ok()?;
    let mut any = false;
    for addr in addrs {
        any = true;
        if !is_public_ip(&addr.ip()) {
            return None;
        }
    }
    if !any { return None; }
    Some(parsed)
}

struct Cache {
    entries: Vec<(String, Instant, Option<LinkPreview>)>,
}

impl Cache {
    fn new() -> Self { Self { entries: Vec::new() } }

    fn get(&mut self, url: &str) -> Option<Option<LinkPreview>> {
        self.entries.retain(|(_, ts, _)| ts.elapsed() < CACHE_TTL);
        self.entries
            .iter()
            .find(|(k, _, _)| k == url)
            .map(|(_, _, v)| v.clone())
    }

    fn put(&mut self, url: String, preview: Option<LinkPreview>) {
        self.entries.retain(|(_, ts, _)| ts.elapsed() < CACHE_TTL);
        if self.entries.len() >= 256 { self.entries.remove(0); }
        self.entries.push((url, Instant::now(), preview));
    }
}

static CLIENT: OnceLock<Client> = OnceLock::new();
static CACHE: OnceLock<Arc<Mutex<Cache>>> = OnceLock::new();

fn client() -> &'static Client {
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(Duration::from_secs(2))
            .redirect(redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 {
                    return attempt.stop();
                }
                if let Some(next) = attempt.url().host_str() {
                    if next.eq_ignore_ascii_case("localhost") {
                        return attempt.stop();
                    }
                }
                // Re-run SSRF validation on the redirect target.
                match validate_url(attempt.url().as_str()) {
                    Some(_) => attempt.follow(),
                    None => attempt.stop(),
                }
            }))
            .user_agent("QxProtocol-LinkPreview/0.1 (+https://github.com/lqxp)")
            .build()
            .expect("failed to build reqwest client")
    })
}

fn cache() -> &'static Arc<Mutex<Cache>> {
    CACHE.get_or_init(|| Arc::new(Mutex::new(Cache::new())))
}

fn decode_html_entities(s: &str) -> String {
    // Minimal decoder — Discord/Twitter titles commonly contain &amp; &quot; &#39; etc.
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    // Case-insensitive attribute match: `attr="..."` or `attr='...'`
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", attr);
    let mut start = lower.find(&needle)?;
    start += needle.len();
    let bytes = tag.as_bytes();
    if start >= bytes.len() { return None; }
    let q = bytes[start];
    if q != b'"' && q != b'\'' { return None; }
    start += 1;
    let slice = &tag[start..];
    let end = slice.find(q as char)?;
    Some(decode_html_entities(&slice[..end]).trim().to_owned())
}

fn parse_og(html: &str, base: &Url) -> Option<LinkPreview> {
    let mut preview = LinkPreview {
        url: base.to_string(),
        ..Default::default()
    };

    for cap in og_meta_regex().captures_iter(html) {
        let tag = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        let prop = extract_attr(tag, "property")
            .or_else(|| extract_attr(tag, "name"))
            .map(|p| p.to_ascii_lowercase())
            .unwrap_or_default();
        let content = extract_attr(tag, "content").unwrap_or_default();
        if content.is_empty() { continue; }

        match prop.as_str() {
            "og:title" | "twitter:title" if preview.title.is_empty() => {
                preview.title = truncate(&content, 300);
            }
            "og:description" | "twitter:description" | "description" if preview.description.is_empty() => {
                preview.description = truncate(&content, 500);
            }
            "og:image" | "twitter:image" | "twitter:image:src" if preview.image.is_empty() => {
                if let Some(resolved) = base.join(&content).ok() {
                    if matches!(resolved.scheme(), "http" | "https") {
                        preview.image = truncate(resolved.as_str(), 600);
                    }
                }
            }
            "og:site_name" if preview.site_name.is_empty() => {
                preview.site_name = truncate(&content, 80);
            }
            _ => {}
        }
    }

    if preview.title.is_empty() {
        if let Some(cap) = title_regex().captures(html) {
            if let Some(m) = cap.get(1) {
                preview.title = truncate(&decode_html_entities(m.as_str().trim()), 300);
            }
        }
    }

    if preview.site_name.is_empty() {
        preview.site_name = base.host_str().unwrap_or("").to_owned();
    }

    if preview.title.is_empty() && preview.description.is_empty() && preview.image.is_empty() {
        return None;
    }

    Some(preview)
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

pub async fn fetch_preview(raw_url: &str) -> Option<LinkPreview> {
    let parsed = validate_url(raw_url)?;
    let cache_key = parsed.to_string();

    {
        let mut cache = cache().lock().await;
        if let Some(cached) = cache.get(&cache_key) {
            return cached;
        }
    }

    let client = client();
    let result = async {
        let mut resp = client.get(parsed.as_str()).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE) {
            let ct = ct.to_str().unwrap_or("").to_ascii_lowercase();
            if !ct.contains("text/html") && !ct.contains("application/xhtml") {
                return None;
            }
        }
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_HTML_BYTES * 2 {
                return None;
            }
        }
        let final_url = resp.url().clone();
        if validate_url(final_url.as_str()).is_none() {
            return None;
        }

        let mut bytes_read = 0usize;
        let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    bytes_read += chunk.len();
                    buf.extend_from_slice(&chunk);
                    if bytes_read >= MAX_HTML_BYTES {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => return None,
            }
        }
        buf.truncate(MAX_HTML_BYTES);
        let html = String::from_utf8_lossy(&buf);
        parse_og(&html, &final_url)
    }
    .await;

    {
        let mut cache = cache().lock().await;
        cache.put(cache_key, result.clone());
    }

    if result.is_none() {
        debug!("no link preview for {}", raw_url);
    }
    result
}
