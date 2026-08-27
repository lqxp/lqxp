# QxChat and QXP-PHANTOM: Protocol Documentation

This directory contains the deep technical reference for the QxChat server and
client, including the QXP-PHANTOM "ghost rendezvous" friend protocol. It is the
source material for the public documentation site.

The documentation describes the protocol as it is implemented in the current
codebase. Where the shipped code differs from the earlier design specification
or the threat model, the difference is called out explicitly rather than
hidden. The canonical gap report is in
[08-protocol-changes.md](./08-protocol-changes.md).

## Scope

The documentation covers:

- The overall client and server architecture and the state model.
- Account registration, login, recovery, and session management.
- The client signature protocol: device signing keys, recovery-word derived
  identity, contextual keys, and the hybrid signature scheme.
- The real-time message transport and the end-to-end encrypted message format.
- The QXP-PHANTOM friend rendezvous protocol: prekeys, slots, envelopes, gates,
  blocking, and the encrypted roster.
- The cryptographic primitives and their exact wire formats.
- The anti-abuse layer: rate limiting, anonymous quota tokens, the Verifiable
  Delay Function, the CAPTCHA challenge, and the Privacy Pass redemption status.

## How to read

| File | Content |
| --- | --- |
| [01-architecture.md](./01-architecture.md) | Components, transport, state model, storage model, and data flow. |
| [02-account-authentication.md](./02-account-authentication.md) | Passwords, sessions, recovery words, and token authentication. |
| [03-client-signature-protocol.md](./03-client-signature-protocol.md) | Device keys, master secret, contextual keys, and hybrid signatures. |
| [04-message-transport.md](./04-message-transport.md) | WebSocket operations, rooms, message delivery, and the E2EE envelope. |
| [05-phantom-protocol.md](./05-phantom-protocol.md) | The QXP-PHANTOM friend rendezvous end to end. |
| [06-cryptographic-primitives.md](./06-cryptographic-primitives.md) | Exact derivations, formats, and byte sizes. |
| [07-anti-abuse.md](./07-anti-abuse.md) | Rate limits, the VDF, CAPTCHA, quota tokens, and Privacy Pass status. |
| [08-protocol-changes.md](./08-protocol-changes.md) | Added operations and the explicit gaps versus the design specification. |

## Conventions

Throughout these documents, the following conventions apply unless a section
says otherwise.

| Symbol | Meaning |
| --- | --- |
| `SHA256(x)` | The raw SHA-256 digest of the byte string `x`. |
| `HKDF(m, salt, info)` | HKDF-SHA256, RFC 5869. |
| `fp(pk)` | `SHA256(pk)` where `pk` is the raw byte encoding of a public key. The result is lowercase hex, 64 characters, and is called a fingerprint. |
| `hex` | Lowercase hexadecimal without a `0x` prefix. |
| `hex64` | A lowercase hexadecimal string of exactly 64 characters (32 bytes). |
| `b64url` | Base64url without padding, RFC 4648 section 5. |

Byte sizes for the post-quantum primitives used by QXP-PHANTOM and the
anti-abuse challenge:

| Primitive | Public or encapsulation key | Ciphertext | Secret or signing key | Signature |
| --- | --- | --- | --- | --- |
| ML-KEM-768 | 1184 bytes | 1088 bytes | 2400 bytes | n/a |
| ML-DSA-65 | 1952 bytes | n/a | n/a | 3309 bytes |

The `ml-kem` (RustCrypto) and `@noble/post-quantum` libraries are used for these
primitives. No polynomial arithmetic is hand implemented in the client or the
server.
