# 06. Cryptographic Primitives

This chapter is a reference for the exact primitives, derivations, constants,
and wire formats used by the server and the client.

## 6.1 Hash and KDF conventions

| Name | Definition |
| --- | --- |
| `SHA256(x)` | Raw SHA-256 digest of bytes `x`. |
| `HKDF(m, salt, info, len)` | HKDF-SHA256 (RFC 5869). |
| `fp(pk)` | `SHA256(raw pk bytes)`, lowercase hex, 64 characters. |
| `hex` | Lowercase hexadecimal, no prefix. |
| `b64url` | Base64url without padding. |
| `||` | Byte concatenation. |

## 6.2 Canonical JSON

Signed payloads are serialized deterministically:

- Object keys sorted recursively.
- Arrays preserved in order.
- Scalars serialized with JSON rules.
- No whitespace.

This contract is implemented in `services/phantom_crypto.rs::canonical_json` and
`crypto/phantom.ts::canonicalJson`. The two implementations are verified against
each other with a cross-language test vector.

## 6.3 Argon2id

Passwords and recovery phrases are hashed with Argon2id using the default
`argon2` crate parameters. Verification uses the same parameters. A fixed dummy
hash is used for constant-time failure when a username or recovery phrase does
not exist.

## 6.4 BIP39 recovery words

Recovery words are generated with `bip39`, 16 bytes of entropy, English
wordlist, which yields 12 words. The phrase is normalized by splitting on
whitespace, lowercasing, and rejoining with single spaces.

## 6.5 HMAC-SHA256

Several subsystems use HMAC-SHA256 with a per-subsystem 32 byte secret generated
once per process by the OS CSPRNG. The HMAC output is lowercase hex. These
secrets are never persisted, so signed tickets and challenges become invalid
after a restart.

The subsystems with their own secret are:

| Subsystem | Secret holder |
| --- | --- |
| Anonymous quota (`rln`) | `core/rln.rs` |
| VDF challenge | `core/vdf.rs` |
| CAPTCHA (`cap`) | `core/cap.rs` |
| Privacy Pass deposit tokens | `services/privacy_pass.rs` |

## 6.6 ECDSA P-256

ECDSA P-256 is used for:

- Device message signing (Web Crypto).
- Contextual friend signing (Web Crypto).
- The ECDSA half of the hybrid signatures.

The signature is the raw `r || s` form (64 bytes, IEEE P1363), not DER. It is
encoded as `b64url`. Verification parses the two 32 byte scalars and verifies
over SHA-256.

## 6.7 ML-DSA-65

ML-DSA-65 (FIPS 204) is the post-quantum half of the hybrid signatures. The
Rust `ml-dsa` crate and the `@noble/post-quantum` `ml-dsa.js` library implement
it.

| Size | Bytes |
| --- | --- |
| Public key | 1952 |
| Signature | 3309 |

## 6.8 ML-KEM-768

ML-KEM-768 (FIPS 203) is used for envelope sealing and the anti-abuse PQC
challenge. The Rust `ml-kem` crate and the `@noble/post-quantum` `ml-kem.js`
library implement it.

| Size | Bytes |
| --- | --- |
| Encapsulation key | 1184 |
| Decapsulation key | 2400 |
| Ciphertext | 1088 |
| Shared secret | 32 |

## 6.9 AES-256-GCM

AES-256-GCM is used for message payloads, the inner envelope, and the roster
blob. The IV is 12 random bytes and the authentication tag is 16 bytes.

## 6.10 Anonymous quota tokens

The `rln` module implements an anonymous, HMAC-based quota token rather than a
zero-knowledge rate-limiting nullifier. The token is:

```json
{ "epoch": 0, "ticket": "<hex>", "signature": "<hex>" }
```

```text
epoch        = floor(now_ms / 15000)
ticket       = HMAC(secret, epoch || ":" || nonce_hex)
signature    = HMAC(secret, epoch || ":" || ticket)
```

The nullifier binds a ticket to an action:

```text
nullifier = SHA256("qxprotocol_rln_nullifier:" || ticket || ":" || epoch_be64 || ":" || action)
```

The server verifies the signature, checks the epoch is within a small window,
compares the nullifier in constant time, and stores it with a 3 minute TTL so
that the same ticket cannot be spent twice for the same action.

## 6.11 Wesolowski Verifiable Delay Function

The VDF is a sequential squaring proof over a fixed 1024 bit RSA modulus. The
modulus is a hardcoded composite of two 512 bit primes. The default iteration
count is 30 000 and the challenge TTL is 3 minutes.

```text
x = HashToElement(target, salt, modulus)
challenge = { modulus, x, t, targetHash, salt, signature, issuedAt, expiresAt }
```

The challenge signature covers `issuedAt:t:targetHash:salt:x:expiresAt`. The
target hash is the first 16 hex characters of `SHA256(target)`. The proof is
verified with the Wesolowski relation:

```text
l = HashToPrime(x, y)         // 128 bit Fiat-Shamir prime
r = 2^t mod l
check: (pi^l * x^r) mod N == y mod N
```

`HashToPrime` derives a 128 bit probable prime from SHA-256 digests over a
counter. `HashToElement` hashes the target and salt into a group element between
2 and the modulus. Both the server verifier and the client solver implement the
same hashes.

## 6.12 Post-quantum challenge

The anti-abuse challenge also includes an ML-KEM-768 encapsulation step. The
server generates an ephemeral keypair, returns the encapsulation key, and later
decapsulates the client ciphertext. The shared secret is discarded after
decapsulation; the step currently acts as a capability gate rather than a key
agreement.

```json
{ "keyId": "<hex>", "ekHex": "<hex 1184 bytes>" }
{ "keyId": "<hex>", "ctHex": "<hex 1088 bytes>" }
```

The ephemeral key has a 3 minute TTL and is consumed once.

## 6.13 CAPTCHA token format

A CAPTCHA challenge is signed with the `cap` HMAC secret. Redeeming it yields a
one-time `cap` token.

```text
capToken = "cap." + tokenId + "." + expiresAt + "." + signature
signature = HMAC(secret, tokenId || ":" || scope || ":" || expiresAt)
```

The token TTL is 5 minutes and it is consumed exactly once.

## 6.14 Privacy Pass deposit token format

The deposit token used by the `pass` gate follows the same structure:

```text
depositToken = "pass." + tokenId + "." + expiresAt + "." + signature
signature = HMAC(secret, tokenId || ":" || expiresAt)
```

The token TTL is 5 minutes. See
[07-anti-abuse.md](./07-anti-abuse.md) for the current redemption status.
