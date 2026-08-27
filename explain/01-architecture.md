# 01. Architecture

## 1.1 System overview

QxChat is a centralized, single-server real-time messaging platform with two
cooperating subsystems:

1. **QxChat core**: rooms, real-time messages, presence, profiles, calls, and
   moderation. This is the existing chat infrastructure.
2. **QXP-PHANTOM**: an add-only friend request protocol that runs on top of the
   core without ever materializing the social graph on the server.

The server is a single Rust binary (`qxprotocol`) exposing an HTTP API and a
WebSocket endpoint. The reference client is a Vue application that runs in the
browser.

```text
+---------------------------+       HTTP (axum)        +---------------------------+
|  Web client (Vue / TS)    | -----------------------> |  Rust server (qxprotocol) |
|                           |                          |                           |
|  - WebSocket socket       | <----------------------> |  - /ws upgrade handler     |
|  - crypto (WebCrypto,     |       WebSocket          |  - account DB (sqlx)       |
|    noble, ml-kem, ml-dsa) |                          |  - room DB (sqlx)          |
|  - localStorage           |                          |  - RAM message store       |
|                           |                          |  - RAM phantom dead-drop   |
+---------------------------+                          +---------------------------+
```

## 1.2 Server modules

The Rust source is split into four areas.

| Directory | Responsibility |
| --- | --- |
| `core/` | Configuration, database, models, presence state, and the crypto gates: `rln`, `vdf`, `cap`, `pqc`, `security`. |
| `services/` | Business logic: `auth`, `messaging`, `room`, `user`, `admin`, `phantom`, `phantom_crypto`, `privacy_pass`. |
| `server/` | HTTP routing (`routes.rs`). |
| `websocket/` | WebSocket connection handling and opcode dispatch (`protocol.rs`). |

The client source is under `web/src/`:

| Directory | Responsibility |
| --- | --- |
| `composables/` | `useMessenger.ts` (session and messaging), `usePhantom.ts` (friend protocol). |
| `crypto/` | `e2ee.ts`, `phantom.ts`, `mldsa.ts`, `pqc.ts`, `rln.ts`, `vdf.ts`. |
| `components/`, `views/`, `i18n/`, `router/` | UI, pages, localization, routing. |

## 1.3 Transport

The client uses two independent transports:

- **Authenticated WebSocket** at `/ws` for chat, rooms, presence, calls, and the
  authenticated QXP-PHANTOM operations (prekey publish/fetch and block updates).
- **Anonymous HTTP** for the QXP-PHANTOM dead-drop operations (`deposit` and
  `poll`) and for the anti-abuse challenge endpoints. These requests never carry
  an authorization header, so the server cannot bind a deposit or a poll to an
  account or to the same session that performed it.

Splitting the dead-drop traffic onto anonymous HTTP is a deliberate privacy
requirement. If deposits and polls were sent over the authenticated WebSocket,
the server would observe the account that deposits into a slot and the account
that later reads the same slot, which would reveal the social edge. The
anonymous endpoints avoid that correlation.

## 1.4 State model

The server keeps live state in `core/presence.rs::AppState`.

| Field | Type | Lifetime |
| --- | --- | --- |
| `players` | `HashMap<session_id, PlayerSession>` | RAM, per connection. |
| `room_messages` | `HashMap<room_id, Vec<ChatMessageRecord>>` | RAM, capped at 150 messages per room, lost on restart. |
| `database` | `RoomDatabase` | Persisted room metadata (title, icon, roles, bans). |
| `accounts` | `AccountDatabase` | Persisted users, sessions, prekeys, blocks, default room, social blobs. |
| `rate_limits` | `HashMap<key, RateLimitBucket>` | RAM, fixed windows. |
| `public_profile_cache` | `HashMap<key, CachedPublicProfile>` | RAM, 5 minute TTL. |
| `call_access_overrides` | `HashSet<String>` | RAM. |

`PlayerSession` tracks the authenticated identity, joined rooms, presence,
call state, and the outbound channel for a single WebSocket connection.

## 1.5 Storage model

The server uses two storage classes.

**RAM only, never persisted:**

- Room messages. Capped at 150 per room (`MAX_ROOM_MESSAGES`), destroyed on
  restart.
- The QXP-PHANTOM dead-drop. Capped at 100 000 envelopes, 24 hour TTL, destroyed
  on restart.
- Rate limit buckets, the public profile cache, and all ephemeral crypto key
  registries (VDF consumed set, PQC ephemeral keys, quota nullifiers, and
  CAPTCHA tokens).

**Persisted in SQLite or PostgreSQL through `sqlx`:**

- `users` (account, password hash, recovery hash, profile, social blob).
- `sessions` (hashed session tokens).
- `feature_flags`.
- `prekeys` (public prekey bundles).
- `blocks` (opaque block tags).
- `default_room` (the single default room record).
- Room metadata in the `RoomDatabase` backend.

The choice between SQLite and PostgreSQL is driven by `database.kind` and
`database.url` in the configuration file.

## 1.6 Configuration

Configuration is read from `files/config.{dev,prod}.toml`, or
`files/config.custom.toml` when it exists. The `PRODUCTION` environment variable
selects the `prod` profile. Configuration covers the API address and port, the
network paths, TURN servers, the database backend, and security flags such as
the admin id list and whether registration is enabled.

Logging is initialized by `core/config.rs::init_tracing` with
`tracing_subscriber` in compact form, filtered by `RUST_LOG`. No IP address or
User-Agent extraction is configured in the tracing layer.

## 1.7 Authentication flow at a glance

Account authentication uses a password for the account and a separate BIP39
recovery phrase for password reset. The details are in
[02-account-authentication.md](./02-account-authentication.md).

The client additionally maintains a cryptographic identity that the server does
not store: a device signing key, an ML-DSA-65 key, an ML-KEM-768 key, and a
master secret derived from the recovery words. This identity is what signs and
seals friend envelopes and message payloads. The details are in
[03-client-signature-protocol.md](./03-client-signature-protocol.md).
