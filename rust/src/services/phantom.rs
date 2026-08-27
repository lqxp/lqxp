use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Once, OnceLock},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::core::{
    models::{
        now_ms, PhantomDepositRequest, PhantomEnvelope, PhantomGateMode, PhantomPollRequest,
        PrekeyBundle,
    },
    presence::SharedState,
    result::{ApiError, ApiResult},
    security::rate_limit_hit,
};

const ENVELOPE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_ENV_PER_SLOT: usize = 16;
const MAX_TOTAL_ENVELOPES: usize = 100_000;
const MAX_SLOTS_PER_POLL: usize = 64;
const MAX_WANT: usize = 8;
const MAX_CT_LEN: usize = 96 * 1024;
const MAX_GATE_TOKEN_LEN: usize = 4096;
const VALID_BUCKETS: &[u32] = &[4096, 16384, 65536];

const GHOST_TOKEN_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_ACTIVE_GHOST_PER_ACCOUNT: usize = 16;

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `tag = SHA256(recipientFp ‖ senderHint)` : barrière de coût côté serveur. Le
/// serveur apprend qu'un couple (fp, hint) est bloqué, jamais quel compte bloque
/// quel compte.
pub fn block_tag(recipient_fp: &str, sender_hint: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(recipient_fp.as_bytes());
    hasher.update(sender_hint.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn validate_envelope(envelope: &PhantomEnvelope) -> ApiResult<()> {
    if envelope.pv != 1 {
        return Err(ApiError::bad_request("Unsupported envelope version."));
    }
    if !is_hex64(&envelope.slot_id)
        || !is_hex64(&envelope.recipient_fp)
        || !is_hex64(&envelope.sender_hint)
    {
        return Err(ApiError::bad_request("Malformed envelope identifier."));
    }
    if !VALID_BUCKETS.contains(&envelope.bucket) {
        return Err(ApiError::bad_request("Invalid envelope bucket."));
    }
    if envelope.ct.is_empty() || envelope.ct.len() > MAX_CT_LEN {
        return Err(ApiError::bad_request("Envelope ciphertext out of bounds."));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

/// `fp(pk) = SHA256(octets bruts de la clé publique)`, hex minuscule 64 chars.
/// Convention partagée serveur/client pour `recipientFp` et le paramètre `f`
/// d'un lien fantôme.
fn fingerprint_of_mlkem_hex(hex: &str) -> Option<String> {
    let bytes = decode_hex(hex)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

// ── Ghost codes (liens d'ami single-use) ─────────────────────────────────────
#[derive(Debug, Clone)]
struct GhostEntry {
    owner_user_id: String,
    expires_at: u64,
}

static GHOST_REGISTRY: OnceLock<Arc<Mutex<HashMap<String, GhostEntry>>>> = OnceLock::new();

fn get_ghost_registry() -> &'static Arc<Mutex<HashMap<String, GhostEntry>>> {
    GHOST_REGISTRY.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn ghost_token_key(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Génère un `ghostToken` (32 octets) et stocke son SHA-256 en RAM (TTL 7 j,
/// max 16 actifs/compte). Renvoie le token encodé b64url (sans padding).
pub async fn register_ghost_token(user_id: &str) -> ApiResult<String> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token_str = URL_SAFE_NO_PAD.encode(bytes);
    let key = ghost_token_key(&bytes);

    let registry = get_ghost_registry();
    let mut map = registry.lock().await;
    let now = now_ms();
    map.retain(|_, entry| entry.expires_at > now);

    let active = map
        .values()
        .filter(|entry| entry.owner_user_id == user_id)
        .count();
    if active >= MAX_ACTIVE_GHOST_PER_ACCOUNT {
        return Err(ApiError::bad_request("Too many active ghost links."));
    }

    map.insert(
        key,
        GhostEntry {
            owner_user_id: user_id.to_owned(),
            expires_at: now + GHOST_TOKEN_TTL_MS,
        },
    );
    Ok(token_str)
}

/// Consomme un ghost token (one-time). Ne retourne rien d'identifiable en cas
/// d'échec — la dépense EST la consommation au dépôt.
pub async fn consume_ghost_token(token: &str) -> ApiResult<()> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| ApiError::bad_request("Invalid ghost token."))?;
    if bytes.len() != 32 {
        return Err(ApiError::bad_request("Invalid ghost token."));
    }
    let key = ghost_token_key(&bytes);
    let registry = get_ghost_registry();
    let mut map = registry.lock().await;
    let now = now_ms();
    match map.remove(&key) {
        Some(entry) if entry.expires_at > now => Ok(()),
        _ => Err(ApiError::bad_request("Ghost token already consumed or expired.")),
    }
}

/// Construit le lien `qxp://ghost#t=<token>&f=<fp(prekey émetteur)>`.
pub async fn create_ghost_link(state: &SharedState, user_id: &str) -> ApiResult<String> {
    let Some(prekey) = state.accounts.get_prekey_by_user_id(user_id).await? else {
        return Err(ApiError::bad_request(
            "Publish a prekey before creating a ghost link.",
        ));
    };
    let bundle: PrekeyBundle = serde_json::from_str(&prekey.bundle_json)
        .map_err(|err| ApiError::internal("Prekey bundle decode", err))?;
    let fingerprint = fingerprint_of_mlkem_hex(&bundle.mlkem768_pk)
        .ok_or_else(|| ApiError::bad_request("Invalid stored prekey."))?;
    let token = register_ghost_token(user_id).await?;
    Ok(format!("qxp://ghost#t={token}&f={fingerprint}"))
}

#[derive(Debug, Clone)]
struct StoredEnvelope {
    envelope: PhantomEnvelope,
    expires_at: u64,
}

/// Boîte aux lettres aveugle, RAM-only, jamais persistée. Chaque enveloppe meurt
/// avec son slot (TTL 24 h). Un redémarrage vide la structure (INV8).
#[derive(Debug, Default)]
pub struct DeadDropStore {
    slots: HashMap<String, VecDeque<StoredEnvelope>>,
}

impl DeadDropStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn live_count(&self) -> usize {
        self.slots.values().map(VecDeque::len).sum()
    }

    pub fn sweep_expired(&mut self, now: u64) -> usize {
        let mut removed = 0usize;
        self.slots.retain(|_, queue| {
            let before = queue.len();
            while queue
                .front()
                .map(|entry| entry.expires_at <= now)
                .unwrap_or(false)
            {
                queue.pop_front();
            }
            removed += before - queue.len();
            !queue.is_empty()
        });
        removed
    }

    pub fn deposit(&mut self, envelope: PhantomEnvelope, now: u64) {
        let slot = envelope.slot_id.clone();
        let entry = StoredEnvelope {
            envelope,
            expires_at: now + ENVELOPE_TTL_MS,
        };
        let queue = self.slots.entry(slot).or_default();
        queue.push_back(entry);
        while queue.len() > MAX_ENV_PER_SLOT {
            queue.pop_front();
        }

        if self.live_count() > MAX_TOTAL_ENVELOPES {
            self.sweep_expired(now);
            while self.live_count() > MAX_TOTAL_ENVELOPES {
                let oldest_slot = self
                    .slots
                    .iter()
                    .filter_map(|(slot, queue)| queue.front().map(|entry| (slot.clone(), entry.expires_at)))
                    .min_by_key(|(_, expires_at)| *expires_at)
                    .map(|(slot, _)| slot);
                let Some(oldest_slot) = oldest_slot else {
                    break;
                };
                if let Some(queue) = self.slots.get_mut(&oldest_slot) {
                    queue.pop_front();
                }
                if self.slots.get(&oldest_slot).map(|q| q.is_empty()) == Some(true) {
                    self.slots.remove(&oldest_slot);
                }
            }
        }
    }

    /// Claim unique par frame : retire l'enveloppe sous lock (consommation).
    pub fn claim(&mut self, slot: &str, now: u64) -> Option<PhantomEnvelope> {
        let queue = self.slots.get_mut(slot)?;
        while queue
            .front()
            .map(|entry| entry.expires_at <= now)
            .unwrap_or(false)
        {
            queue.pop_front();
        }
        let entry = queue.pop_front()?;
        if queue.is_empty() {
            self.slots.remove(slot);
        }
        Some(entry.envelope)
    }
}

static PHANTOM_STORE: OnceLock<Arc<Mutex<DeadDropStore>>> = OnceLock::new();
static SWEEP_STARTED: Once = Once::new();

fn get_store() -> &'static Arc<Mutex<DeadDropStore>> {
    PHANTOM_STORE.get_or_init(|| {
        let store = Arc::new(Mutex::new(DeadDropStore::new()));
        SWEEP_STARTED.call_once(|| {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let removed = store.lock().await.sweep_expired(now_ms());
                    if removed > 0 {
                        tracing::debug!("phantom dead-drop sweep removed {} envelopes", removed);
                    }
                }
            });
        });
        store
    })
}

pub async fn deposit(state: &SharedState, req: PhantomDepositRequest) -> ApiResult<()> {
    if rate_limit_hit(state, "phantom:deposit:global".to_string(), 20, 1_000).await {
        return Err(ApiError::too_many_requests("Deposit rate limit exceeded."));
    }

    validate_envelope(&req.envelope)?;

    if !req.gate.nullifier.is_empty() && !is_hex64(&req.gate.nullifier) {
        return Err(ApiError::bad_request("Malformed gate nullifier."));
    }
    if req.gate.token.len() > MAX_GATE_TOKEN_LEN {
        return Err(ApiError::bad_request("Gate token out of bounds."));
    }

    // Barrière de coût serveur : rejet silencieux d'un couple (fp, hint) bloqué.
    let tag = block_tag(&req.envelope.recipient_fp, &req.envelope.sender_hint);
    if state.accounts.is_blocked_tag(&tag).await? {
        return Err(ApiError::forbidden("Deposit rejected."));
    }

    // Gating ordonné : 2) nullifier RLN lié au jour epoch, 3) mode.
    let action = format!("phantom_deposit:{}", now_ms() / 86_400_000);
    let quota_token = req
        .gate
        .quota_token
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("Missing anonymous quota token."))?;
    crate::core::rln::verify_and_consume_nullifier(quota_token, &req.gate.nullifier, &action)
        .await?;

    match req.gate.mode {
        PhantomGateMode::Cap => {
            crate::core::cap::verify_and_consume_cap_token(&req.gate.token, "phantom").await?;
        }
        PhantomGateMode::Pass => {
            crate::services::privacy_pass::consume_deposit_token(&req.gate.token).await?;
        }
        PhantomGateMode::Ghost => {
            consume_ghost_token(&req.gate.token).await?;
        }
    }

    get_store().lock().await.deposit(req.envelope, now_ms());
    Ok(())
}

pub async fn poll(
    state: &SharedState,
    req: PhantomPollRequest,
) -> ApiResult<Vec<Option<PhantomEnvelope>>> {
    if req.slots.len() > MAX_SLOTS_PER_POLL {
        return Err(ApiError::bad_request("Too many slots requested."));
    }
    let want = req.want.clamp(0, MAX_WANT);

    if rate_limit_hit(state, "phantom:poll:global".to_string(), 20, 1_000).await {
        return Err(ApiError::too_many_requests("Poll rate limit exceeded."));
    }

    let store = get_store();
    let mut guard = store.lock().await;
    let now = now_ms();

    let mut frames = Vec::with_capacity(want);
    for slot in req.slots.iter().take(want) {
        frames.push(guard.claim(slot, now));
    }
    while frames.len() < want {
        frames.push(None);
    }
    Ok(frames)
}

pub async fn fetch_prekey(state: &SharedState, username: &str) -> ApiResult<Option<PrekeyBundle>> {
    if rate_limit_hit(state, "phantom:prekey:global".to_string(), 60, 60_000).await {
        return Err(ApiError::too_many_requests("Prekey lookup rate limit exceeded."));
    }

    let Some(stored) = state.accounts.get_prekey_by_username(username).await? else {
        return Ok(None);
    };

    let bundle: PrekeyBundle = serde_json::from_str(&stored.bundle_json)
        .map_err(|err| ApiError::internal("Prekey bundle decode", err))?;
    Ok(Some(bundle))
}

/// Op 36 — publie un bundle de prékey après vérification des DEUX signatures
/// hybrides (ECDSA P-256 ‖ ML-DSA-65) sur la forme canonique.
pub async fn publish_prekey(
    state: &SharedState,
    user_id: &str,
    bundle: &PrekeyBundle,
) -> ApiResult<serde_json::Value> {
    crate::services::phantom_crypto::verify_prekey_bundle(bundle)?;

    let bundle_json = serde_json::to_string(bundle)
        .map_err(|err| ApiError::internal("Prekey bundle encode", err))?;
    state.accounts.publish_prekey(user_id, &bundle_json).await?;

    Ok(json!({ "ok": true, "version": bundle.version }))
}

/// Op 37 — récupère les bundles publics d'un lot d'usernames (≤8).
pub async fn fetch_prekeys(
    state: &SharedState,
    usernames: &[String],
) -> ApiResult<serde_json::Value> {
    let mut bundles = serde_json::Map::new();
    for username in usernames {
        if let Some(stored) = state.accounts.get_prekey_by_username(username).await? {
            if let Ok(bundle) = serde_json::from_str::<PrekeyBundle>(&stored.bundle_json) {
                bundles.insert(username.clone(), serde_json::to_value(bundle).unwrap_or(serde_json::Value::Null));
            }
        }
    }
    Ok(json!({ "bundles": bundles }))
}

async fn owner_fingerprint(state: &SharedState, user_id: &str) -> ApiResult<String> {
    let prekey = state
        .accounts
        .get_prekey_by_user_id(user_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("Publish a prekey before updating blocks."))?;
    let bundle: PrekeyBundle = serde_json::from_str(&prekey.bundle_json)
        .map_err(|err| ApiError::internal("Prekey bundle decode", err))?;
    fingerprint_of_mlkem_hex(&bundle.mlkem768_pk)
        .ok_or_else(|| ApiError::bad_request("Invalid stored prekey."))
}

/// Op 39 — bloque/débloque des hints de façon opaque. Le serveur stocke
/// `SHA256(fp(mlkem_pk_propriétaire) ‖ hint)` ; il ne joint jamais compte→cible.
pub async fn update_blocks(
    state: &SharedState,
    user_id: &str,
    add: &[String],
    remove: &[String],
) -> ApiResult<serde_json::Value> {
    let owner_fp = owner_fingerprint(state, user_id).await?;

    for hint in add {
        if !is_hex64(hint) {
            return Err(ApiError::bad_request("Malformed block hint."));
        }
        state
            .accounts
            .add_block_tag(user_id, &block_tag(&owner_fp, hint))
            .await?;
    }
    for hint in remove {
        if !is_hex64(hint) {
            return Err(ApiError::bad_request("Malformed block hint."));
        }
        state
            .accounts
            .remove_block_tag(user_id, &block_tag(&owner_fp, hint))
            .await?;
    }

    let filter = state.accounts.list_block_tags(user_id).await?;
    Ok(json!({ "filter": filter }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_tag_is_deterministic_and_order_sensitive() {
        let a = block_tag(&"a".repeat(64), &"b".repeat(64));
        let b = block_tag(&"a".repeat(64), &"b".repeat(64));
        let swapped = block_tag(&"b".repeat(64), &"a".repeat(64));
        assert_eq!(a, b);
        assert_ne!(a, swapped);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn envelope_validation_rejects_bad_hex() {
        let mut env = PhantomEnvelope {
            pv: 1,
            slot_id: "0".repeat(64),
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 16384,
            ct: "abc".to_string(),
        };
        assert!(validate_envelope(&env).is_ok());

        env.slot_id = "zz".into();
        assert!(validate_envelope(&env).is_err());
    }

    #[test]
    fn envelope_validation_rejects_bad_bucket() {
        let env = PhantomEnvelope {
            pv: 1,
            slot_id: "0".repeat(64),
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 1234,
            ct: "abc".to_string(),
        };
        assert!(validate_envelope(&env).is_err());
    }

    #[test]
    fn dead_drop_claims_once_and_evicts_oldest() {
        let mut store = DeadDropStore::new();
        let env = |n: u8| PhantomEnvelope {
            pv: 1,
            slot_id: "a".repeat(64),
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 4096,
            ct: format!("ct-{n}"),
        };

        store.deposit(env(1), 0);
        store.deposit(env(2), 0);
        assert_eq!(store.live_count(), 2);

        let claimed = store.claim(&"a".repeat(64), 0).expect("first claim");
        assert_eq!(claimed.ct, "ct-1");
        assert_eq!(store.live_count(), 1);
    }

    #[test]
    fn dead_drop_expires_by_ttl() {
        let mut store = DeadDropStore::new();
        let env = PhantomEnvelope {
            pv: 1,
            slot_id: "a".repeat(64),
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 4096,
            ct: "x".to_string(),
        };
        store.deposit(env, 0);
        assert!(store.claim(&"a".repeat(64), 0).is_some());
        // Après TTL + 1 ms, plus rien n'est servable.
        let env = PhantomEnvelope {
            pv: 1,
            slot_id: "b".repeat(64),
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 4096,
            ct: "y".to_string(),
        };
        store.deposit(env, 0);
        assert!(store.claim(&"b".repeat(64), ENVELOPE_TTL_MS + 1).is_none());
    }
}
