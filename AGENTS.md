# AGENTS.md — lqxp protocol server

Reference server for the LQXP protocol (`qxprotocol` binary, Rust + Axum).
Blind relay: the server routes ciphertext and enforces limits — it never sees
plaintext and stores no message content.

## Layout

- `rust/src/websocket/protocol.rs` — all WS ops (`process_message` dispatch).
- `rust/src/websocket/mod.rs` — connections, heartbeat timeout, disconnect.
- `rust/src/core/` — presence/state, security (rate limits, usernames),
  database, config, crypto helpers (`vdf.rs`, `pqc.rs`, `cap.rs`, `rln.rs`).
- `rust/src/services/`, `rust/src/server/`, `rust/src/web/` — REST, phantom,
  web-client checkout builder.
- `explain/` — protocol spec, source material for the public wiki.
- `docs/`, `deploy/`, `scripts/` — Docker/TURN/systemd/PM2/Pelican recipes.
- `files/config.custom.toml` (gitignored) — the only production config.

## Build / test / lint

```sh
cargo check
cargo test          # must stay 34/34 green
cargo clippy --all-targets   # pre-existing warnings only; add none
```

No test harness for WS handlers exists; behavior changes to the relay must at
minimum compile, pass `cargo test`, and keep clippy warning count flat.

## Rust conventions

- WS errors: static `&'static str` via `respond_error(state, sid, op, msg,
  req_id)` (`protocol.rs`). Never interpolate user input into error strings.
- `requestId` is capped at 128 chars server-side (`utils.rs`, S5). Acks echo
  it via `with_request_id`.
- `client_id`/`platform` always go through `sanitize_client_id` /
  `sanitize_platform`. Never trust raw values for routing.
- Locks: collect `tx` clones under the `players` read lock, `try_send`
  **after** dropping it. Never hold a lock across `.await` sends.
- Relay fan-out is lossy by design (512-msg queue); new relay acks must
  report `delivered`/`dropped` instead of unconditional `{ ok: true }`.
- Rate limits via `rate_limit_hit(state, "scope:session:{sid}", n, window_ms)`.
  Mesh/burst paths need headroom (e.g. cloud_sync 120/10s); the global WS
  guard (1200/60s in `websocket/mod.rs`) stays the abuse backstop.
- `process_message` rejects unknown ops with op 0; server→client-only events
  use fresh opcodes and must be ignored safely by old clients.

## Protocol changes (new op checklist)

1. Add dispatch arm in `process_message` + handler following an existing
   relay (`relay_cloud_sync_op`, `relay_room_signal`) as template.
2. Update the opcode table in `explain/04-message-transport.md`.
3. Document wire format, limits, and client healing contract in the relevant
   `explain/0X-*.md`; record spec-vs-shipped diffs in `08-protocol-changes.md`.
4. Mirror the change to the wiki (see Docs below) in the same PR.
5. Keep old clients working: never alter an existing `op 61`-style payload
   shape; only extend acks with additive fields.

## Signing / crypto rules (audited properties — do not regress)

- Anything hashed by both peers (KDF transcripts, hello canonical bytes) must
  use canonical JSON (sorted keys), never `JSON.stringify`-equivalent
  (`serde_json::to_string` on a `BTreeMap`/sorted `Map`). The server
  re-serializes sorted — naive hashing diverges per side.
- Secret comparisons (nullifiers, hashes, HMACs) must be constant-time.
- VDF challenge signatures cover `expiresAt` (S2); VDF params are
  length-bounded before bigint parsing (S3).
- Hybrid signatures (ECDSA P-256 + ML-DSA/SLH-DSA) verify **both**, fail
  closed; handshake-only for PQ signatures (17 KiB each would eat the 64 KiB
  relay budget).
- Session data stays AES-256-GCM; room keys never travel raw (wrap per peer).
- The relay stays amnesic: no DB write, no dead-drop, no logging of
  `encrypted` payloads for sync/signal ops.

## Languages / translations

- Server error strings and logs: English, static. Code comments may be French
  (existing style) — keep them short, never chain-of-thought.
- User-facing translations live in `lqxp/client` (`useI18n`), NOT here. Do
  not add server-side i18n; the server only emits stable English error keys.
- Username rules: 2–32 chars (24 at registration), reserved-name + leet
  variants blocked (`core/security.rs`); keep in sync with the client's
  `RESERVED_USERNAMES`.

## Deploy

- Bare metal: `./update.sh` (refuses detached HEAD, requires
  `files/qxp.sqlite`, `cargo build --release`, PM2 restart). Never commit
  `files/qxp.sqlite`, uploads, or `config.custom.toml`.
- Docker: `docker compose up -d --build` (`:4560`, mounts: custom TOML ro,
  sqlite, database.json). TURN secrets via env (`QXP_TURN_*`), never in git.
- First boot with empty DB refuses to start by design: set
  `createIfMissing = true` once, start, then remove it.
- Guides: `docs/docker-deployment.md`, `docs/turn-deployment.md`.

## Docs / wiki sync

`explain/` is the spec source of truth AND the wiki source. After any protocol
change, update in the same PR:

1. `explain/*.md` here.
2. `site/explain/*.md` mirror (copy the updated file).
3. `site/app/components/wiki/WikiSections.vue` — the wiki renders hardcoded
   sections (`/wiki/<section>`), not the markdown. Update the matching
   `<article>` + the opcode table in `message-transport`.

## Git

- Branch `main`; commit messages: `feat|fix|docs|chore(scope): subject`.
- Push per repo; `lqxp/client` and `lqxp/app` are separate checkouts with
  their own remotes — a sync change usually spans server + client + site.
- Never force-push `main`.
