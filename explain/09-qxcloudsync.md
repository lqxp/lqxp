# 09. QxCloudSync Protocol

Device-to-device sync (rooms, messages, parameters, friend-room keys) between
two sessions of the same `user_id`, in `deepMerge` mode. The server is a
**blind, amnesic relay**: it stores nothing, reads nothing, logs nothing.

## 9.1 Transport

- Client → server: `{ "op": 60, "d": { "toClientId": "<48 chars|empty=broadcast>",
  "encrypted": { ...opaque... }, "requestId?": "<≤128 chars>" } }`
- Server → siblings: `{ "op": 61, "d": { "fromClientId": "...",
  "toClientId": "...", "encrypted": { ...opaque, forwarded as-is... } } }`
- Server → sender (only if `requestId` present):
  `{ "op": 60, "d": { "ok": true, "requestId": "..." } }` (relay ack, not peer ack).

Handler: `relay_cloud_sync_op` (`websocket/protocol.rs`), modelled on
`relay_room_signal` (op 55→56):

- Rate limit `cloud_sync:session:{sid}` 30/10s. Global WS guard 1200/60s applies.
- `d.encrypted` must be a JSON object, serialized length ≤ 64 KiB
  (`MAX_CLOUD_SYNC_BYTES`), else `"Payload too large"`.
- Sender must be identified (`user_id` + `username` non-empty; `revalidate_session`
  already enforced by dispatch).
- Fan-out: `players` with same `user_id`, `session_id != self`,
  `(toClientId == "" || client_id == toClientId)`. Collected under the read lock,
  `try_send` after the lock is dropped (lossy if a 512-message queue is full,
  same as op 55/111).
- No DB write, no `room_messages` push, no dead-drop, no history. Invisible
  devices are included (unlike room broadcasts).
- Stale routing is silent: a `toClientId` that no longer maps to a connected
  session yields zero recipients, yet the sender still gets `{ ok: true }`
  (relay ack, not peer ack). The client therefore never trusts the relay ack
  and heals routing itself (see §9.3).

## 9.2 Trust root and key schedule (client-only)

- `masterSecret` from the 12 recovery words (PBKDF2-SHA256 100k,
  salt `qxphantom:master` → HKDF `qxp-master`), never transmitted.
- `syncRoot = HKDF(master, "", "qxcloudsync:root:v1")`,
  `syncAuth = HKDF(syncRoot, "", "qxcloudsync:auth:v1")` (HMAC key for hellos).
- Handshake (all inside `d.encrypted`, signed): `hello` (eph P-256 + ephemeral
  ML-KEM-768 pk) → `accept` (+ ML-KEM ct to initiator) → `confirm` (+ ML-KEM ct
  to responder). Each hello carries `auth = HMAC(syncAuth, canonical)` plus a
  **hybrid signature**: device ECDSA P-256 **and** SLH-DSA-SHA2-128f (FIPS 205,
  17088-byte signatures, `crypto/slhdsa.ts`). Both must verify (fail closed);
  a wrong HMAC or a missing/invalid PQ signature is silently dropped. Each
  device holds a long-term SLH-DSA identity keypair (`qxcloudsync-device-v1`).
- Post-quantum posture: KEX = ECDH + 2× ML-KEM-768 (FIPS 203), safe via the
  ML-KEM component; identity = ECDSA + SLH-DSA (FIPS 205); session data =
  AES-256-GCM (PQ-safe symmetric, Grover halves 256-bit to a comfortable
  128-bit). PQ signatures are deliberately handshake-only: a 17 KiB signature
  on every data part would eat the 64 KiB relay budget and CPU per push, while
  AES-GCM already gives PQ authenticity per part.
- `syncMaster = HKDF(ecdh || ss1 || ss2 || syncRoot, transcript,
  "qxcloudsync:master:v1")`, RAM-only. Ephemeral private keys wiped after use.
  Sessions are additionally persisted client-side, AES-GCM encrypted under
  `HKDF(syncRoot, "", "qxcloudsync:persist:v1")`, so a browser restart does not
  force a re-pair (the server still stores nothing).
- Topology is a full mesh of pairwise sessions with **no transitive trust
  and no forwarding**: A paired with B and C does not make B and C know each
  other, and A never relays between them. Every pair needs its own direct
  handshake (hello → accept → confirm → dedicated master/epoch). N devices =
  N(N−1)/2 handshakes. Version vectors still gossip-merge, so data converges
  once the pairwise legs exist.
- Pairing: with sync enabled and the 12 words present, a client with zero
  sessions broadcasts a signed hello on boot (jittered 2–6 s); any holder of
  the same words verifies the HMAC and answers. Devices that already hold
  sessions do not auto-hello; the manual Pair button (or a re-handshake, see
  §9.3) covers later joins. Hello broadcast, accept/confirm/data unicast via
  `toClientId`. Duplicate hellos are ignored while the existing session shows
  recent verified inbound traffic, but answered when that session is stale
  (reinstall / divergence recovery) — the completed handshake then replaces
  the old session and its secrets are wiped.
- Hellos carry a signed normalized `platform` (`mobile` | `web` | `desktop`)
  shown in Settings → Sync with one icon per type.
- Epoch keys: `syncEpochKey_k = HKDF(syncMaster, BE64(k),
  "qxcloudsync:epoch:v1")`, TTL 7 days, auto-rotation at TTL − 10% via a signed
  `rekey` hello (no new ECDH). Old epoch key dropped.
- Data envelopes: AES-256-GCM under the epoch key (AAD
  `syncId:epoch:n:from:to`) + device ECDSA signature. Anti-replay on
  `(syncId, epoch, n)`.
- `roomKey` never travels raw: `AES-GCM(wrapKey, roomKey)` where
  `wrapKey = HKDF(epochKey, "", "qxcloudsync:roomkey-wrap:v1")`.

## 9.3 Mesh self-healing (client-only)

`peerWs` (the peer's WS `client_id` used for unicast) goes stale on every
reconnect, tab reload, or phone wake — and the relay drops such frames
without telling the sender (§9.1). Before healing, a stale leg stalled
silently (auto-hello only fires with zero sessions) and a rekey lost in a
stale route split epochs permanently (both sides then drop everything:
the sender already advanced, the receiver's own rekey is rejected as
`epoch != sess.epoch + 1`).

- `lastInboundAt` per session: last *verified inbound* traffic only.
  (`lastSeen` is also bumped on send, so it can never detect a one-way
  black hole — a sender pushing into the void would look "fresh".)
- Stale route → broadcast fallback: past 3 min without inbound, pushes,
  rekeys and revokes go out with empty `toClientId` (server fans out to all
  same-`user_id` sessions). Reception refreshes `peerWs` via `fromClientId`
  and the ack heals the way back — one round trip restores the leg.
- Epoch healing on receive, only for envelopes addressed to us (`outer.to`
  must equal our device id; third-party broadcast fallbacks are ignored):
  missed rekey → bounded local fast-forward (≤ 10 epochs; the subsequent
  `openData` authenticates, so only the master holder can trigger adoption);
  peer behind → resend of the current signed rekey; unknown `syncId` →
  fresh handshake. Re-handshakes and rekey resends are rate-limited to
  1/min/peer.
- Epoch is never advanced without a successful rekey send
  (`sendRekeyMessage`, shared by the rotation scheduler and the resend path).

## 9.4 deepMerge rules

Every object carries `{ updatedAt, by }`; rooms/messages carry ids.
Version vectors `{ deviceId: counter }` select deltas; merge = union + LWW
(tie-break: lexicographically greater `by` wins). Ratchets merge by `max`.
Trusted sender keys merge by union; a divergent JWK keeps the local one and
raises an error (no silent overwrite). **Room-key conflict policy: refuse +
manual choice** — the local key is kept, the conflict is surfaced in
Settings → Sync, the user picks Keep local / Request remote re-push.

Large snapshots are split into valid sub-snapshots (rooms/params chunk +
message batches of ~100/room) so each relay frame stays ≤ 64 KiB. Full history
lives in client IndexedDB (`qxcloudsync-v1`), beyond the 500/room
localStorage cap. Sync pauses under client-lock, RAM-only OPSEC, or decoy.

Room deletion (op 57/58) travels as a `deleted` tombstone collection (30-day
TTL): the deleted room is dropped locally (lists, messages, keys, pins,
IndexedDB) and never re-imported from stale snapshots while the tombstone
lives. Pins (`pinnedRooms`, ≤5) sync LWW; room `members` are attached for
newly imported rooms only, so live rosters are never clobbered.

## 9.5 Event propagation
Snapshots are full-state and idempotent, but they are not only periodic:
`persist()` itself notifies subscribers (internal mutation calls included),
coalesced into one push 2.5 s after the last change. Out-of-band stores
(custom theme, locale) are observed with synchronous watchers. Applying a
remote snapshot never re-notifies (internal guard), so there is no echo loop.
The 90 s timer remains as a safety net.

## 9.6 Security properties (audited)

- **Quantum-proof (hybrid):** KEX = ECDH P-256 + 2× ML-KEM-768 (FIPS 203),
  safe via the ML-KEM component; identity = ECDSA P-256 + SLH-DSA-SHA2-128f
  (FIPS 205), both required; session data = AES-256-GCM (≈128-bit PQ margin).
- **Transcript integrity:** the KDF transcript is hashed over canonical JSON
  (sorted keys), never `JSON.stringify` — the server re-serializes in sorted
  order, so a naive hash would diverge per side.
- **Anti-replay:** per-peer `(syncId, epoch)` window with monotonic high-water
  (tolerance 5000 for reordering) plus a bounded dedup set (6000 entries,
  pruned). Survives restarts via the persisted `sendN`; LWW merge makes
  residual replays harmless.
- **At rest:** without client lock, session blobs are AES-GCM under a
  syncRoot-derived key (device boundary, same as the stored recovery words).
  With client lock active, sessions and the SLH-DSA identity rest only under
  AES-GCM envelopes keyed by the lock key; locking wipes all RAM secrets
  (masters, epoch keys, syncRoot, SLH cache) and never downgrades envelopes.
- **Residual (accepted):** hello replay is a bounded nuisance (server rate
  30/10 s, 5 recent pendings max, 120 s sweep); the server observes timing,
  frame counts and approximate sizes (no padding to fixed buckets, unlike
  PHANTOM); PBKDF2-100k for the master follows the pre-existing PHANTOM
  parameters.
