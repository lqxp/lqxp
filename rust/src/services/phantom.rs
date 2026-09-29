use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Once, OnceLock},
};

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
/// Per-envelope cap (base64 of ct): must cover the largest bucket.
/// Bucket 65536 → 1088 (ML-KEM-768) + 12 (IV) + 65536 + 16 (tag) = 66652
/// bytes → ~88872 in base64. A lower cap (e.g. 8 KiB) would reject
/// ALL signed requests (the ML-DSA-65 signature alone is already
/// 3309 bytes). The global memory budget stays bounded by MAX_TOTAL_BYTES.
/// (INV: any signed intro/welcome exceeds the 4096 bucket.)
const MAX_CT_LEN: usize = 96 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_GATE_TOKEN_LEN: usize = 4096;
const VALID_BUCKETS: &[u32] = &[4096, 16384, 65536];

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `tag = SHA256(recipientFp ‖ senderHint)`: server-side cost barrier. The
/// server learns that an (fp, hint) pair is blocked, never which account blocks
/// which account.
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

/// `fp(pk) = SHA256(raw public-key bytes)`, lowercase hex, 64 chars.
/// Shared server/client convention for `recipientFp`.
fn fingerprint_of_mlkem_hex(hex: &str) -> Option<String> {
    let bytes = decode_hex(hex)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Clone)]
struct StoredEnvelope {
    envelope: PhantomEnvelope,
    expires_at: u64,
}

/// Blind mailbox, RAM-only, never persisted. Each envelope dies
/// with its slot (24 h TTL). A restart empties the store (INV8).
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

    pub fn live_bytes(&self) -> usize {
        self.slots
            .values()
            .flat_map(|queue| queue.iter())
            .map(|entry| entry.envelope.ct.len())
            .sum()
    }

    pub fn try_deposit(&mut self, envelope: PhantomEnvelope, now: u64) -> bool {
        if self.live_bytes().saturating_add(envelope.ct.len()) > MAX_TOTAL_BYTES {
            return false;
        }
        self.deposit(envelope, now);
        true
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

    /// Single claim per frame: removes the envelope under lock (consume).
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

    // Server-side cost barrier: silent reject of a blocked (fp, hint) pair.
    let tag = block_tag(&req.envelope.recipient_fp, &req.envelope.sender_hint);
    if state.accounts.is_blocked_tag(&tag).await? {
        return Err(ApiError::forbidden("Deposit rejected."));
    }

    // Ordered gating: 2) day-epoch-bound RLN nullifier, 3) mode.
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
    }

    if !get_store()
        .lock()
        .await
        .try_deposit(req.envelope, now_ms())
    {
        return Err(ApiError::too_many_requests(
            "Dead-drop memory budget reached.",
        ));
    }
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

/// Op 36 — publishes a prekey bundle after verifying BOTH hybrid
/// signatures (ECDSA P-256 ‖ ML-DSA-65) over the canonical form.
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

/// Op 37 — fetches public bundles for a batch of usernames (≤8).
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

/// Op 39 — blocks/unblocks hints opaquely. The server stores
/// `SHA256(fp(owner_mlkem_pk) ‖ hint)`; it never joins account→target.
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
        // After TTL + 1 ms, nothing is servable anymore.
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

    // ── Stresser PHANTOM : dead-drop sous charge + gates + fautes ──────────
    // The global relay store is process-shared: every handler test below uses
    // unique slots (uuid-prefixed) so parallel tests never cross-talk.

    fn stress_slot(run: &str, n: u32) -> String {
        format!("{run}{n:032x}")
    }

    fn stress_run() -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }

    fn stress_envelope(slot: String, n: u32) -> PhantomEnvelope {
        PhantomEnvelope {
            pv: 1,
            slot_id: slot,
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 4096,
            ct: format!("ct-{n}"),
        }
    }

    #[test]
    fn dead_drop_per_slot_cap_is_fifo_16() {
        let mut store = DeadDropStore::new();
        let slot = "a".repeat(64);
        for n in 0..20u32 {
            store.deposit(stress_envelope(slot.clone(), n), 0);
        }
        assert_eq!(store.live_count(), MAX_ENV_PER_SLOT);
        // Oldest 4 evicted: first claim returns #4.
        let first = store.claim(&slot, 0).expect("claim");
        assert_eq!(first.ct, "ct-4");
    }

    #[test]
    fn dead_drop_cross_slot_isolation() {
        let mut store = DeadDropStore::new();
        store.deposit(stress_envelope("a".repeat(64), 1), 0);
        assert!(store.claim(&"b".repeat(64), 0).is_none());
        assert_eq!(store.live_count(), 1);
        assert!(store.claim(&"a".repeat(64), 0).is_some());
        assert_eq!(store.live_count(), 0);
    }

    #[test]
    fn dead_drop_sweep_removes_only_expired() {
        let mut store = DeadDropStore::new();
        store.deposit(stress_envelope("a".repeat(64), 1), 0);
        store.deposit(stress_envelope("b".repeat(64), 2), 0);
        // Sweep halfway: nothing expires yet.
        assert_eq!(store.sweep_expired(ENVELOPE_TTL_MS - 1), 0);
        assert_eq!(store.live_count(), 2);
        // Past TTL: everything goes, empty slots vanish.
        assert_eq!(store.sweep_expired(ENVELOPE_TTL_MS + 1), 2);
        assert_eq!(store.live_count(), 0);
    }

    #[tokio::test]
    async fn dead_drop_concurrent_deposits_stay_capped() {
        use std::sync::Arc;
        let store = Arc::new(tokio::sync::Mutex::new(DeadDropStore::new()));
        let mut tasks = Vec::new();
        for t in 0..8u32 {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                for n in 0..250u32 {
                    let slot = format!("slot-{t:02}");
                    let mut guard = store.lock().await;
                    guard.deposit(stress_envelope(slot, n), 0);
                }
            }));
        }
        for task in tasks {
            task.await.expect("worker");
        }
        // 8 slots x 250 deposits each, per-slot FIFO cap 16.
        assert_eq!(store.lock().await.live_count(), 8 * MAX_ENV_PER_SLOT);
    }

    async fn stress_state() -> crate::core::presence::SharedState {
        use crate::core::config::{Config, DatabaseConfig};
        use crate::core::database::{AccountDatabase, RoomDatabase};
        use crate::core::models::now_ms;
        use crate::core::presence::{AppState, RuntimeCounters};
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc;
        use tokio::sync::{Mutex, RwLock};
        let dir = std::env::temp_dir().join(format!("lqxp-phantom-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temporary test dir");
        let url = format!("sqlite://{}/test.sqlite?mode=rwc", dir.display());
        let db_cfg = DatabaseConfig {
            kind: "sqlite".to_owned(),
            url,
            create_if_missing: true,
        };
        let accounts = AccountDatabase::connect(&db_cfg, vec![], true)
            .await
            .expect("connect test accounts db");
        let database = RoomDatabase::connect(&db_cfg)
            .await
            .expect("connect test room db");
        Arc::new(AppState {
            config: Config::default(),
            started_at_ms: now_ms(),
            runtime: Arc::new(RuntimeCounters::default()),
            blocklist_terms: Arc::new(vec![]),
            players: Arc::new(RwLock::new(HashMap::new())),
            room_messages: Arc::new(RwLock::new(HashMap::new())),
            database: Arc::new(database),
            accounts: Arc::new(accounts),
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            public_profile_cache: Arc::new(Mutex::new(HashMap::new())),
            call_access_overrides: Arc::new(RwLock::new(HashSet::new())),
            poll_tallies: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    // PhantomGate is the only model type not already in scope via `super::*`.
    use crate::core::models::PhantomGate;

    fn stress_gate(token: String, nullifier: String, quota: crate::core::rln::EpochQuotaToken) -> PhantomGate {
        PhantomGate {
            mode: PhantomGateMode::Cap,
            token,
            nullifier,
            quota_token: Some(quota),
        }
    }

    async fn stress_quota() -> (crate::core::rln::EpochQuotaToken, String) {
        use crate::core::rln::{compute_nullifier, current_epoch, generate_quota_token};
        let token = generate_quota_token();
        let action = format!("phantom_deposit:{}", now_ms() / 86_400_000);
        let nullifier = compute_nullifier(&token.ticket, token.epoch, &action);
        assert_eq!(nullifier.len(), 64);
        assert!(current_epoch() >= token.epoch);
        (token, nullifier)
    }

    async fn stress_cap_token() -> String {
        crate::core::cap::mint_test_cap_token(
            &uuid::Uuid::new_v4().simple().to_string(),
            "phantom",
            600_000,
        )
        .await
    }

    #[tokio::test]
    async fn deposit_validation_matrix_rejects_before_gates() {
        let state = stress_state().await;
        let run = stress_run();
        let good_slot = stress_slot(&run, 1);
        let (quota, nullifier) = stress_quota().await;
        let base = PhantomEnvelope {
            pv: 1,
            slot_id: good_slot,
            recipient_fp: "f".repeat(64),
            sender_hint: "e".repeat(64),
            bucket: 16384,
            ct: "QUJD".to_owned(),
        };
        // Control: structurally valid envelope passes validation (fails later
        // at the cap-token gate with a dummy token — proving validation ran).
        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: base.clone(),
                gate: stress_gate("tok".to_owned(), nullifier.clone(), quota.clone()),
            },
        )
        .await
        .expect_err("dummy cap token must fail");
        assert!(err.to_string().contains("CAPTCHA"), "unexpected: {err}");

        let cases: Vec<(&str, PhantomEnvelope)> = vec![
            ("bad pv", PhantomEnvelope { pv: 2, ..base.clone() }),
            ("bad slot", PhantomEnvelope { slot_id: "zz".to_owned(), ..base.clone() }),
            ("bad fp", PhantomEnvelope { recipient_fp: "0".repeat(63), ..base.clone() }),
            ("bad bucket", PhantomEnvelope { bucket: 8192, ..base.clone() }),
            ("empty ct", PhantomEnvelope { ct: String::new(), ..base.clone() }),
            ("oversize ct", PhantomEnvelope { ct: "A".repeat(96 * 1024 + 1), ..base.clone() }),
        ];
        for (label, envelope) in cases {
            let err = deposit(
                &state,
                PhantomDepositRequest {
                    envelope,
                    gate: stress_gate("tok".to_owned(), nullifier.clone(), quota.clone()),
                },
            )
            .await
            .unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("version")
                    || msg.contains("identifier")
                    || msg.contains("bucket")
                    || msg.contains("bounds"),
                "{label}: unexpected error {msg}"
            );
        }

        // Missing quota token and malformed nullifier fail before the store.
        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: base.clone(),
                gate: PhantomGate {
                    mode: PhantomGateMode::Cap,
                    token: "tok".to_owned(),
                    nullifier: nullifier.clone(),
                    quota_token: None,
                },
            },
        )
        .await
        .expect_err("missing quota must fail");
        assert!(err.to_string().contains("quota"));

        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: base,
                gate: PhantomGate {
                    mode: PhantomGateMode::Cap,
                    token: "tok".to_owned(),
                    nullifier: "not-hex".to_owned(),
                    quota_token: Some(quota),
                },
            },
        )
        .await
        .expect_err("malformed nullifier must fail");
        assert!(err.to_string().contains("nullifier"));
    }

    #[tokio::test]
    async fn deposit_poll_gated_roundtrip_with_consume_and_padding() {
        let state = stress_state().await;
        let run = stress_run();
        let slot = stress_slot(&run, 7);
        let (quota, nullifier) = stress_quota().await;
        let token = stress_cap_token().await;
        deposit(
            &state,
            PhantomDepositRequest {
                envelope: stress_envelope(slot.clone(), 1),
                gate: stress_gate(token, nullifier, quota),
            },
        )
        .await
        .expect("gated deposit");

        // First poll claims the envelope…
        let frames = poll(
            &state,
            PhantomPollRequest {
                slots: vec![slot.clone()],
                want: 1,
            },
        )
        .await
        .expect("poll");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().expect("claimed").ct, "ct-1");

        // …second poll finds nothing (consumed) and pads to `want` with nulls.
        let frames = poll(
            &state,
            PhantomPollRequest {
                slots: vec![slot, "0".repeat(64)],
                want: 3,
            },
        )
        .await
        .expect("poll");
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.is_none()));

        // `want` is clamped to 8.
        let frames = poll(&state, PhantomPollRequest { slots: vec![], want: 100 })
            .await
            .expect("poll");
        assert_eq!(frames.len(), 8);

        // Too many slots rejected.
        let err = poll(
            &state,
            PhantomPollRequest {
                slots: vec!["0".repeat(64); 65],
                want: 1,
            },
        )
        .await
        .expect_err("slot overflow must fail");
        assert!(err.to_string().contains("slots"));
    }

    #[tokio::test]
    async fn deposit_replays_and_double_spends_fail() {
        let state = stress_state().await;
        let run = stress_run();
        // Same quota token twice: nullifier already consumed.
        let (quota, nullifier) = stress_quota().await;
        let t1 = stress_cap_token().await;
        deposit(
            &state,
            PhantomDepositRequest {
                envelope: stress_envelope(stress_slot(&run, 1), 1),
                gate: stress_gate(t1, nullifier.clone(), quota.clone()),
            },
        )
        .await
        .expect("first deposit");
        let t2 = stress_cap_token().await;
        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: stress_envelope(stress_slot(&run, 2), 2),
                gate: stress_gate(t2, nullifier, quota),
            },
        )
        .await
        .expect_err("nullifier replay must fail");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("replay")
                || msg.contains("nullifier")
                || msg.contains("quota")
                || msg.contains("429"),
            "unexpected: {msg}"
        );

        // Same cap token twice: single-use.
        let (quota2, nullifier2) = stress_quota().await;
        let t3 = stress_cap_token().await;
        deposit(
            &state,
            PhantomDepositRequest {
                envelope: stress_envelope(stress_slot(&run, 3), 3),
                gate: stress_gate(t3.clone(), nullifier2.clone(), quota2.clone()),
            },
        )
        .await
        .expect("first use");
        let (quota3, nullifier3) = stress_quota().await;
        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: stress_envelope(stress_slot(&run, 4), 4),
                gate: stress_gate(t3, nullifier3, quota3),
            },
        )
        .await
        .expect_err("cap replay must fail");
        assert!(err.to_string().contains("consumed"));
    }

    #[tokio::test]
    async fn deposit_blocked_pair_rejected_opaquely() {
        let state = stress_state().await;
        let (user, _, _) = state
            .accounts
            .register("ghost_block", "password123")
            .await
            .expect("register");
        let fp = "b".repeat(64);
        let hint = "c".repeat(64);
        let tag = block_tag(&fp, &hint);
        assert!(state.accounts.add_block_tag(&user.id, &tag).await.expect("block"));
        let run = stress_run();
        let (quota, nullifier) = stress_quota().await;
        let token = stress_cap_token().await;
        let err = deposit(
            &state,
            PhantomDepositRequest {
                envelope: PhantomEnvelope {
                    pv: 1,
                    slot_id: stress_slot(&run, 1),
                    recipient_fp: fp,
                    sender_hint: hint,
                    bucket: 4096,
                    ct: "QUJD".to_owned(),
                },
                gate: stress_gate(token, nullifier, quota),
            },
        )
        .await
        .expect_err("blocked pair must fail");
        // Opaque: identical message no matter which side blocks
        // (Display prefixes the status code).
        assert!(err.to_string().ends_with("Deposit rejected."), "unexpected: {err}");
    }
}
