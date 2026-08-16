use argon2::{
    password_hash::{
        rand_core::OsRng as PasswordOsRng, PasswordHash, PasswordHasher, PasswordVerifier,
        SaltString,
    },
    Argon2,
};
use bip39::Language;
use rand::{distributions::Alphanumeric, rngs::OsRng, thread_rng, Rng, RngCore};
use sha2::{Digest, Sha256};

use crate::core::result::{ApiError, ApiResult};

const USERNAME_MIN: usize = 2;
const USERNAME_MAX: usize = 32;
const RESERVED_USERNAMES: &[&str] = &["system"];
const PASSWORD_MIN: usize = 8;
const PASSWORD_MAX: usize = 128;
const RECOVERY_WORD_COUNT: usize = 16;

pub fn hash_secret(secret: &str) -> ApiResult<String> {
    let salt = SaltString::generate(&mut PasswordOsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| ApiError::internal("password hashing", err))
}

pub fn verify_secret(secret: &str, hash: &str) -> ApiResult<()> {
    let parsed = PasswordHash::new(hash)
        .map_err(|_| ApiError::unauthorized("Invalid credentials."))?;
    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .map_err(|_| ApiError::unauthorized("Invalid credentials."))
}

static DUMMY_HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn get_dummy_password_hash() -> &'static str {
    DUMMY_HASH.get_or_init(|| {
        hash_secret("claiming_0_ai_while_dropping_gpt_punctuation_enjoy_the_200ms_argon2_burn_and_3_new_regressions_:^)").unwrap_or_else(|_| {
            "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQxMjM0NTY3OA$f0lV9mK2N9yCjH+mE7P6aQ1sW2eR3tY4uI5oP6aQ7s8".to_string()
        })
    })
}

pub fn verify_secret_constant_time(secret: &str, real_hash: Option<&str>) -> ApiResult<()> {
    match real_hash {
        Some(hash) => verify_secret(secret, hash),
        None => {
            let _ = verify_secret(secret, get_dummy_password_hash());
            Err(ApiError::unauthorized("Invalid credentials."))
        }
    }
}

pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn generate_session_token() -> String {
    let mut rng = thread_rng();
    std::iter::repeat_with(|| rng.sample(Alphanumeric))
        .map(char::from)
        .take(64)
        .collect()
}

pub fn generate_snowflake_id() -> String {
    let mut rng = thread_rng();
    let random_id: u64 = rng.gen_range(100_000_000_000_000_000..=999_999_999_999_999_999);
    random_id.to_string()
}

pub fn generate_recovery_words() -> Vec<String> {
    let mut entropy = [0u8; 20];
    OsRng.fill_bytes(&mut entropy);
    let mnemonic = bip39::Mnemonic::from_entropy_in(Language::English, &entropy)
        .expect("valid entropy for BIP39 mnemonic");
    mnemonic
        .to_string()
        .split_whitespace()
        .take(RECOVERY_WORD_COUNT)
        .map(str::to_owned)
        .collect()
}

pub fn normalize_username(username: &str) -> String {
    username.trim().to_lowercase()
}

pub fn normalize_recovery_phrase(words: &str) -> String {
    words
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn validate_username(username: &str) -> ApiResult<String> {
    let trimmed = username.trim();
    let char_count = trimmed.chars().count();
    if !(USERNAME_MIN..=USERNAME_MAX).contains(&char_count) {
        return Err(ApiError::bad_request(format!(
            "Username must be between {USERNAME_MIN} and {USERNAME_MAX} characters."
        )));
    }

    let normalized = normalize_username(trimmed);
    if RESERVED_USERNAMES.contains(&normalized.as_str()) {
        return Err(ApiError::bad_request("Username is reserved."));
    }

    let valid = trimmed.chars().all(|ch| {
        ch.is_alphanumeric()
            || matches!(
                ch,
                '_' | '.' | '-' | ' ' | '#' | '@' | '\'' | '\"' | '(' | ')' | '[' | ']' | '!' | '?'
            )
    });
    if !valid {
        return Err(ApiError::bad_request(
            "Username contains invalid characters.",
        ));
    }

    Ok(trimmed.to_owned())
}

pub fn validate_password(password: &str) -> ApiResult<()> {
    let char_count = password.chars().count();
    if !(PASSWORD_MIN..=PASSWORD_MAX).contains(&char_count) {
        return Err(ApiError::bad_request(format!(
            "Password must be between {PASSWORD_MIN} and {PASSWORD_MAX} characters."
        )));
    }
    Ok(())
}

pub fn username_hits_blocklist(username: &str, blocklist: &[String]) -> bool {
    let normalized = normalize_username(username);
    blocklist.iter().any(|term| {
        let term_norm = normalize_username(term);
        !term_norm.is_empty() && normalized.contains(&term_norm)
    })
}

pub fn sanitize_filename(value: &str, fallback: &str, max_len: usize) -> String {
    let mut sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    sanitized = sanitized.trim_matches('.').trim().to_owned();
    if sanitized.is_empty() {
        sanitized = fallback.to_owned();
    }
    sanitized.chars().take(max_len).collect()
}

pub async fn rate_limit_hit(
    state: &crate::core::presence::AppState,
    key: impl Into<String>,
    limit: u32,
    window_ms: u64,
) -> bool {
    let now = crate::core::models::now_ms();
    let key = key.into();
    let mut buckets = state.rate_limits.lock().await;
    if buckets.len() > 5_000 {
        buckets.retain(|_, bucket| now.saturating_sub(bucket.window_start_ms) <= bucket.window_ms);
    }
    let bucket = buckets
        .entry(key)
        .or_insert(crate::core::presence::RateLimitBucket {
            window_start_ms: now,
            window_ms,
            count: 0,
        });
    if now.saturating_sub(bucket.window_start_ms) > window_ms {
        bucket.window_start_ms = now;
        bucket.window_ms = window_ms;
        bucket.count = 0;
    }
    bucket.count = bucket.count.saturating_add(1);
    bucket.count > limit
}
