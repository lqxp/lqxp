# 07. Anti-Abuse

The anti-abuse layer combines fixed-window rate limits, an anonymous quota
token, a Verifiable Delay Function, an ML-KEM post-quantum challenge, and an
optional CAPTCHA token. Its purpose is to make mass account creation and mass
deposits expensive without linking the work to a user account.

## 7.1 Rate limiting

`core/security.rs::rate_limit_hit` implements a keyed fixed-window counter. A
bucket holds a window start, a window length, and a count. Buckets are evicted
when the store exceeds 5000 entries.

Representative limits:

| Key | Limit | Window |
| --- | --- | --- |
| `phantom:deposit:global` | 20 | 1 second |
| `phantom:poll:global` | 20 | 1 second |
| `phantom:prekey:global` | 60 | 60 seconds |
| `prekey-fetch:user:<id>` | 4 | 15 seconds |
| `register:global` | 10 | 15 seconds |
| `register:user:<name>` | 3 | 30 seconds |
| `recover:user:<name>` | 3 | 30 seconds |
| `auth:cap:challenge:global` | 60 | 10 seconds |
| `auth:challenge:global` | 60 | 10 seconds |
| `pass:redeem:global` | 60 | 60 seconds |
| `chat:session:<id>` | 30 | 60 seconds |

In addition, chat messages enforce a 400 millisecond minimum interval per
session.

## 7.2 Anonymous quota token

Before any gated action, the client requests an anonymous quota token from
`GET /api/auth/challenge`. The token is an HMAC-signed epoch ticket. The client
computes an action-scoped nullifier and the server consumes it, preventing
reuse of the same ticket for the same action.

The token and nullifier format is described in
[06-cryptographic-primitives.md](./06-cryptographic-primitives.md).

The nullifier store is RAM-only, capped at 10 000 entries, with a 3 minute TTL.
The nullifier comparison is constant time.

## 7.3 Verifiable Delay Function

The VDF forces a sequential squaring computation before a gated action. The
client must compute `y = x^(2^t) mod N` and produce the Wesolowski proof `pi`.
The server verifies the proof in logarithmic time. The default iteration count
is 30 000 and the challenge expires after 3 minutes.

The VDF parameters and verification equation are in
[06-cryptographic-primitives.md](./06-cryptographic-primitives.md).

## 7.4 Post-quantum challenge

The challenge also carries an ephemeral ML-KEM-768 encapsulation key. The client
encapsulates and returns the ciphertext; the server decapsulates to confirm the
client performed the KEM step. The key is one-time and expires after 3 minutes.

## 7.5 CAPTCHA flow

The full CAPTCHA flow returns a one-time `cap` token that can gate a registration,
login, recovery, or deposit.

```text
client                          server
  |  GET /api/auth/cap/challenge  |
  |----------------------------->|
  |  { challengeId, scope,        |
  |    issuedAt, expiresAt,       |
  |    vdf, quotaToken, pqcKey,   |
  |    signature }                |
  |<-----------------------------|
  |  solve VDF                    |
  |  compute nullifier            |
  |  encapsulate ML-KEM           |
  |  POST /api/auth/cap/redeem    |
  |  { challenge, vdfProof,       |
  |    nullifier, pqcCiphertext } |
  |----------------------------->|
  |  { success, capToken }        |
  |<-----------------------------|
```

The redeem handler verifies the challenge signature, consumes the quota
nullifier, verifies the VDF proof, and decapsulates the ML-KEM ciphertext. It
also checks optional instrumentation: an automated browser flag and a minimum
interaction time of 250 milliseconds.

The `cap` token is HMAC-signed, expires after 5 minutes, and is consumed once.

## 7.6 Privacy Pass redemption

`POST /api/pass/redeem` is wired and implements the nonce store with a reserve,
verify, commit, release cycle. The store is capped at 100 000 entries with FIFO
eviction. The deposit token issuance and consumption are implemented.

However, the actual VOPRF verification of the `AmortizedBatchTokenResponse`
against a public issuer keyset is not yet wired. The `verify_amortized_batch_response`
function currently returns an error unconditionally. As a result, the `pass`
deposit gate cannot currently be satisfied. The `cap` gate is the working path.

## 7.7 Storage bounds and TTLs

The ephemeral stores are bounded to prevent unbounded growth.

| Store | Cap | TTL |
| --- | --- | --- |
| Room messages | 150 per room | n/a, lost on restart |
| Phantom dead-drop | 16 per slot, 100 000 total | 24 hours |
| Anonymous nullifiers | 10 000 | 3 minutes |
| VDF consumed challenges | 10 000 | challenge expiry |
| PQC ephemeral keys | 10 000 | 3 minutes |
| CAPTCHA tokens | pruned by expiry | 5 minutes |
| Privacy Pass nonce store | 100 000 | n/a, FIFO eviction |
