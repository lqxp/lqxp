use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::core::{
    models::now_ms,
    result::{ApiError, ApiResult},
};

type HmacSha256 = Hmac<Sha256>;

const CHALLENGE_TTL_MS: u64 = 3 * 60 * 1000;
pub const DEFAULT_REGISTER_DIFFICULTY: u32 = 18;
pub const DEFAULT_LOGIN_DIFFICULTY: u32 = 14;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoWChallenge {
    pub challenge: String,
    pub signature: String,
    pub difficulty: u32,
    pub expires_at: u64,
}

static SERVER_HMAC_SECRET: OnceLock<[u8; 32]> = OnceLock::new();
static NULLIFIERS: OnceLock<Arc<Mutex<HashMap<String, u64>>>> = OnceLock::new();

fn get_hmac_secret() -> &'static [u8; 32] {
    SERVER_HMAC_SECRET.get_or_init(|| {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    })
}

fn get_nullifiers() -> &'static Arc<Mutex<HashMap<String, u64>>> {
    NULLIFIERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn sign_challenge(challenge: &str) -> String {
    let secret = get_hmac_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(challenge.as_bytes());
    let result = mac.finalize();
    format!("{:x}", result.into_bytes())
}

fn verify_signature(challenge: &str, signature: &str) -> bool {
    let secret = get_hmac_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(challenge.as_bytes());
    let expected = format!("{:x}", mac.finalize().into_bytes());
    
    if expected.len() != signature.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(signature.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn generate_challenge(action: &str, difficulty_override: Option<u32>) -> PoWChallenge {
    let now = now_ms();
    let expires_at = now + CHALLENGE_TTL_MS;
    let difficulty = difficulty_override.unwrap_or(match action {
        "register" => DEFAULT_REGISTER_DIFFICULTY,
        _ => DEFAULT_LOGIN_DIFFICULTY,
    });
    let mut salt_bytes = [0u8; 8];
    OsRng.fill_bytes(&mut salt_bytes);
    let salt = format!("{:x}", u64::from_le_bytes(salt_bytes));

    let challenge = format!("{now}:{action}:{salt}:{difficulty}");
    let signature = sign_challenge(&challenge);

    PoWChallenge {
        challenge,
        signature,
        difficulty,
        expires_at,
    }
}

pub fn count_leading_zero_bits(bytes: &[u8]) -> u32 {
    let mut zeros = 0u32;
    for &byte in bytes {
        if byte == 0 {
            zeros += 8;
        } else {
            zeros += byte.leading_zeros();
            break;
        }
    }
    zeros
}

pub async fn verify_pow(
    challenge: &str,
    signature: &str,
    nonce: u64,
    expected_action: &str,
) -> ApiResult<()> {
    if !verify_signature(challenge, signature) {
        return Err(ApiError::bad_request("Invalid security challenge signature."));
    }

    let parts: Vec<&str> = challenge.split(':').collect();
    if parts.len() != 4 {
        return Err(ApiError::bad_request("Malformed security challenge."));
    }

    let timestamp: u64 = parts[0]
        .parse()
        .map_err(|_| ApiError::bad_request("Malformed challenge timestamp."))?;
    let action = parts[1];
    let difficulty: u32 = parts[3]
        .parse()
        .map_err(|_| ApiError::bad_request("Malformed challenge difficulty."))?;

    if action != expected_action {
        return Err(ApiError::bad_request("Challenge action mismatch."));
    }

    let now = now_ms();
    if now.saturating_sub(timestamp) > CHALLENGE_TTL_MS {
        return Err(ApiError::bad_request("Security challenge expired. Please retry."));
    }
    if timestamp > now + 30_000 {
        return Err(ApiError::bad_request("Challenge timestamp in the future."));
    }

    let mut hasher = Sha256::new();
    hasher.update(challenge.as_bytes());
    hasher.update(b":");
    hasher.update(nonce.to_string().as_bytes());
    let hash_bytes = hasher.finalize();

    let leading_zeros = count_leading_zero_bits(&hash_bytes);
    if leading_zeros < difficulty {
        return Err(ApiError::bad_request(format!(
            "Insufficient proof-of-work (got {leading_zeros} zeros, required {difficulty})."
        )));
    }

    let nullifier = format!("{:x}", hash_bytes);
    let nullifiers_arc = get_nullifiers();
    let mut nullifiers = nullifiers_arc.lock().await;

    if nullifiers.len() > 10_000 {
        nullifiers.retain(|_, expires| *expires > now);
    }

    if let Some(&expires) = nullifiers.get(&nullifier) {
        if expires > now {
            return Err(ApiError::bad_request("Security challenge already used."));
        }
    }

    nullifiers.insert(nullifier, now + CHALLENGE_TTL_MS);

    Ok(())
}
