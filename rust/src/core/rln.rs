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

pub const EPOCH_WINDOW_MS: u64 = 15_000;
const NULLIFIER_TTL_MS: u64 = 3 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpochQuotaToken {
    pub epoch: u64,
    pub ticket: String,
    pub signature: String,
}

static SERVER_RLN_SECRET: OnceLock<[u8; 32]> = OnceLock::new();
static RLN_NULLIFIERS: OnceLock<Arc<Mutex<HashMap<String, u64>>>> = OnceLock::new();

fn get_rln_secret() -> &'static [u8; 32] {
    SERVER_RLN_SECRET.get_or_init(|| {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    })
}

fn get_rln_nullifiers() -> &'static Arc<Mutex<HashMap<String, u64>>> {
    RLN_NULLIFIERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn sign_ticket(ticket_data: &str) -> String {
    let secret = get_rln_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(ticket_data.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn verify_ticket_signature(ticket_data: &str, signature: &str) -> bool {
    let secret = get_rln_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take any key size");
    mac.update(ticket_data.as_bytes());
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

pub fn current_epoch() -> u64 {
    now_ms() / EPOCH_WINDOW_MS
}

pub fn generate_quota_token() -> EpochQuotaToken {
    let epoch = current_epoch();
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let nonce_hex = format!("{:x}", u128::from_le_bytes(nonce));
    let data = format!("{epoch}:{nonce_hex}");
    let ticket = sign_ticket(&data);
    let signature = sign_ticket(&format!("{epoch}:{ticket}"));

    EpochQuotaToken {
        epoch,
        ticket,
        signature,
    }
}

pub fn compute_nullifier(ticket: &str, epoch: u64, action: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"qxprotocol_rln_nullifier:");
    h.update(ticket.as_bytes());
    h.update(b":");
    h.update(epoch.to_be_bytes());
    h.update(b":");
    h.update(action.as_bytes());
    format!("{:x}", h.finalize())
}

pub async fn verify_and_consume_nullifier(
    token: &EpochQuotaToken,
    nullifier: &str,
    expected_action: &str,
) -> ApiResult<()> {
    let now = now_ms();
    let curr_epoch = current_epoch();

    if token.epoch > curr_epoch + 1 || curr_epoch.saturating_sub(token.epoch) > 4 {
        return Err(ApiError::bad_request(
            "Anonymous quota token expired. Please obtain a fresh token.",
        ));
    }

    let data = format!("{}:{}", token.epoch, token.ticket);
    if !verify_ticket_signature(&data, &token.signature) {
        return Err(ApiError::bad_request("Invalid anonymous quota token signature."));
    }

    let expected_nullifier = compute_nullifier(&token.ticket, token.epoch, expected_action);
    if nullifier != expected_nullifier {
        return Err(ApiError::bad_request("Malformed rate-limiting nullifier."));
    }

    let nullifiers_arc = get_rln_nullifiers();
    let mut nullifiers = nullifiers_arc.lock().await;

    if nullifiers.len() > 10_000 {
        nullifiers.retain(|_, expires| *expires > now);
    }

    if let Some(&expires) = nullifiers.get(nullifier) {
        if expires > now {
            return Err(ApiError::too_many_requests(
                "Rate limit quota exceeded for this epoch. Please wait.",
            ));
        }
    }

    nullifiers.insert(nullifier.to_owned(), now + NULLIFIER_TTL_MS);

    Ok(())
}
