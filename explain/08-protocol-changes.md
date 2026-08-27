# 08. Protocol Changes and Gap Report

This chapter records the operations added for QXP-PHANTOM, the feature
additions, and the explicit differences between the earlier design
specification or threat model and the shipped implementation.

## 8.1 Added WebSocket operations

Three authenticated WebSocket operations were added for QXP-PHANTOM:

| Opcode | Name | Request | Response |
| --- | --- | --- | --- |
| 36 | Publish prekey | a prekey bundle | `{ "ok": true, "version": 1 }` |
| 37 | Fetch prekeys | `{ "usernames": ["..."] }` | `{ "bundles": { "user": bundle } }` |
| 39 | Update blocks | `{ "add": ["<hex64>"], "remove": ["<hex64>"] }` | `{ "filter": ["<hex64>"] }` |

All three require an identified session and follow the existing
`respond_error(state, sid, op, message, request_id)` error pattern with static
messages.

## 8.2 Opcode renumbering versus the specification

The design specification assigned PHANTOM the opcodes 36, 37, and 45. In the
shipped code, opcode 45 was already occupied by moderation (kick), so block
updates use 39. Prekey publish and fetch keep their specification opcodes.

| Concept | Specification opcode | Shipped opcode |
| --- | --- | --- |
| Publish prekey | 36 | 36 |
| Fetch prekeys | 37 | 37 |
| Update blocks | 45 | 39 |

## 8.3 Default room

A single default room record was added so the server can offer an onboarding or
announcement room. The record is stored in the `default_room` table with a fixed
row id of 1 and contains `roomId`, `roomKey`, and `title`.

- Admin endpoint: `POST /api/admin/default-room` with `{ roomId, roomKey, title }`
  or `{ clear: true }`.
- The identification response (opcode 2) includes the record as `defaultRoom`.
- The client persists an `allowServerDefaultRoom` toggle and a per-room
  `defaultRoomLeavedRoomId` flag. The client auto-joins on identification unless
  the user disabled the behavior or already left that specific room.

## 8.4 Social blob

The encrypted roster sync was added as `GET /api/social/blob` and
`PUT /api/social/blob`. The blob is an AES-256-GCM ciphertext produced by the
client; the server only enforces the last-write-wins version and the 64 KiB size
limit.

## 8.5 Post-quantum challenge migration

The anti-abuse PQC challenge was migrated from a hand implemented polynomial
KEM to the audited `ml-kem` (RustCrypto) and `@noble/post-quantum` libraries.
The wire fields changed from `{ tHex, rhoHex }` / `{ uHex, vHex }` to
`{ ekHex }` / `{ ctHex }`. The server and client were updated together.

## 8.6 Gap report

The following table records where the threat model or the design document
describes a mechanism that differs from what is shipped. These are intentional
accuracy notes, not an indication that the implementation is silently weaker
without review.

| Area | Design or threat model claim | Shipped implementation | Impact |
| --- | --- | --- | --- |
| Message encryption | MLS with X-Wing hybrid post-quantum (E8) | Room-key AES-256-GCM with ECDSA P-256 signatures (`QXDR-A256GCM-HKDFSHA256`), no MLS, no X-Wing | Messages are end-to-end encrypted but not post-quantum and not MLS. |
| Rate-limiting nullifier | Zero-knowledge RLN (E5) | HMAC-signed anonymous quota tokens and SHA-256 nullifiers | Anonymous and replay-resistant, but not zero-knowledge in the SNARK sense. |
| Privacy Pass redemption | Implemented (S1, E7) | Route and nonce store wired, but VOPRF verification is a stub that always fails | The `pass` deposit gate is currently unusable. |
| Contextual pseudonym | Ed25519 derived per room (P3) | ECDSA P-256 derived per room | Same domain separation property, different curve. |
| Inner envelope sealing | MLS / X-Wing inner layer (P4) | ML-KEM-768 plus AES-256-GCM, no second MLS layer | Envelopes are sealed to the recipient and opaque to the server. |
| Deposit gate | Pass, cap, or ghost token | Anonymous quota nullifier plus one of pass or cap | One additional anonymous gate is always required. The ghost gate was removed. |

## 8.7 Audit items integrated

Several audit items are present in the implementation:

- The VDF challenge signature covers `expiresAt` in addition to the other fields
  (S2).
- VDF parameters are length-bounded before parsing into large integers (S3).
- Nullifier and target-hash comparisons use constant-time checks (S4).
- Device and contextual signatures use ECDSA P-256 rather than Ed25519 (C3).
