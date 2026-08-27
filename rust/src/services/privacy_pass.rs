use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::core::{
    models::now_ms,
    result::{ApiError, ApiResult},
};

type HmacSha256 = Hmac<Sha256>;

const PASS_DEPOSIT_TOKEN_TTL_MS: u64 = 5 * 60 * 1000;
const MAX_NONCE_STORE_ENTRIES: usize = 100_000;
const MAX_TOKEN_RESPONSE_LEN: usize = 64 * 1024;

static PASS_SECRET: OnceLock<[u8; 32]> = OnceLock::new();
static CONSUMED_DEPOSIT_TOKENS: OnceLock<Arc<Mutex<HashMap<String, u64>>>> = OnceLock::new();
static NONCE_STORE: OnceLock<Arc<Mutex<MemoryNonceStore>>> = OnceLock::new();

fn get_pass_secret() -> &'static [u8; 32] {
    PASS_SECRET.get_or_init(|| {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    })
}

fn get_consumed_store() -> &'static Arc<Mutex<HashMap<String, u64>>> {
    CONSUMED_DEPOSIT_TOKENS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn get_nonce_store() -> &'static Arc<Mutex<MemoryNonceStore>> {
    NONCE_STORE.get_or_init(|| Arc::new(Mutex::new(MemoryNonceStore::new())))
}

fn sign_data(data: &str) -> String {
    let secret = get_pass_secret();
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC key is valid");
    mac.update(data.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonceState {
    Reserved,
    Committed,
}

#[derive(Debug)]
struct NonceEntry {
    state: NonceState,
    seq: u64,
}

/// Store de nonces à usage unique pour la redemption Privacy Pass. Plafonné à
/// 100 k entrées avec éviction FIFO (corrige l'audit #7).
#[derive(Debug, Default)]
pub struct MemoryNonceStore {
    entries: HashMap<String, NonceEntry>,
    next_seq: u64,
}

impl MemoryNonceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Réserve un nonce. Renvoie `false` s'il est déjà utilisé ou si le store
    /// est plein et que l'éviction échoue.
    pub fn reserve(&mut self, nonce: &str) -> bool {
        if self.entries.contains_key(nonce) {
            return false;
        }
        while self.entries.len() >= MAX_NONCE_STORE_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.seq)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            nonce.to_owned(),
            NonceEntry {
                state: NonceState::Reserved,
                seq: self.next_seq,
            },
        );
        self.next_seq = self.next_seq.wrapping_add(1);
        true
    }

    pub fn commit(&mut self, nonce: &str) -> bool {
        match self.entries.get_mut(nonce) {
            Some(entry) if entry.state == NonceState::Reserved => {
                entry.state = NonceState::Committed;
                true
            }
            _ => false,
        }
    }

    pub fn release(&mut self, nonce: &str) {
        if let Some(entry) = self.entries.get(nonce) {
            if entry.state == NonceState::Reserved {
                self.entries.remove(nonce);
            }
        }
    }
}

/// Délivre un jeton de dépôt éphémère (HMAC, 5 min, consommable une fois). Il
/// évite qu'un même pass soit rejoué plusieurs fois.
pub async fn issue_deposit_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let token_id = format!("{:x}", u128::from_le_bytes(bytes));
    let expires_at = now_ms() + PASS_DEPOSIT_TOKEN_TTL_MS;
    let data = format!("{token_id}:{expires_at}");
    let signature = sign_data(&data);
    let token = format!("pass.{token_id}.{expires_at}.{signature}");

    let store = get_consumed_store();
    store.lock().await.insert(token.clone(), expires_at);
    token
}

pub async fn consume_deposit_token(token: &str) -> ApiResult<()> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 4 || parts[0] != "pass" {
        return Err(ApiError::bad_request("Invalid deposit token format."));
    }
    let token_id = parts[1];
    let expires_at = parts[2]
        .parse::<u64>()
        .map_err(|_| ApiError::bad_request("Invalid deposit token timestamp."))?;
    let signature = parts[3];

    let now = now_ms();
    if now > expires_at {
        return Err(ApiError::bad_request("Deposit token expired."));
    }

    let data = format!("{token_id}:{expires_at}");
    let expected = sign_data(&data);
    if !constant_time_eq(&expected, signature) {
        return Err(ApiError::bad_request("Invalid deposit token signature."));
    }

    let store = get_consumed_store();
    let mut map = store.lock().await;
    map.retain(|_, exp| *exp > now);
    match map.remove(token) {
        Some(exp) if exp > now => Ok(()),
        _ => Err(ApiError::bad_request("Deposit token already consumed or invalid.")),
    }
}

/// Vérifie un `AmortizedBatchTokenResponse` Privacy Pass contre le keyset
/// public de l'émetteur, en passant par le cycle reserve → verify → commit.
///
/// S1 : la vérification VOPRF réelle (RFC 9578, Ristretto255) requiert un
/// émetteur Privacy Pass (clé + endpoint d'émission + rotation) qui n'est pas
/// encore câblé. Échec fermé tant que ce n'est pas en place — on ne réinvente
/// pas la primitive.
pub async fn redeem_pass_token(token_response: &str, nonce: &str) -> ApiResult<String> {
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request("Malformed redemption nonce."));
    }
    if token_response.is_empty() || token_response.len() > MAX_TOKEN_RESPONSE_LEN {
        return Err(ApiError::bad_request("Token response out of bounds."));
    }

    let store = get_nonce_store();
    let mut nonces = store.lock().await;
    if !nonces.reserve(nonce) {
        return Err(ApiError::bad_request("Redemption nonce already used."));
    }

    if let Err(err) = verify_amortized_batch_response(token_response) {
        nonces.release(nonce);
        return Err(err);
    }
    nonces.commit(nonce);
    drop(nonces);

    Ok(issue_deposit_token().await)
}

/// Point de couture : vérification VOPRF finalize (RFC 9578) contre le keyset
/// public. À câbler dans un jalon crypto dédié avec un crate audité.
fn verify_amortized_batch_response(_token_response: &str) -> ApiResult<()> {
    Err(ApiError::bad_request(
        "Privacy Pass redemption is not yet available.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_store_reserve_commit_release() {
        let mut store = MemoryNonceStore::new();
        assert!(store.reserve("a"));
        assert!(!store.reserve("a")); // déjà réservé
        assert!(store.commit("a"));
        assert!(!store.commit("a")); // déjà commité

        assert!(store.reserve("b"));
        store.release("b");
        assert!(store.reserve("b")); // libéré, donc réutilisable
    }
}
