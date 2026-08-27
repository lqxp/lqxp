# 05. QXP-PHANTOM Protocol

QXP-PHANTOM is an add-only friend request protocol. Its central property is that
the social graph never exists on the server. A request is delivered only into a
blind mailbox slot derived from a secret shared by the two parties, and there is
no negative response: ignoring a request simply lets it expire.

## 5.1 Privacy goals

The design targets these properties:

- The server cannot reconstruct a social edge from its own state.
- A deposit and a poll are not bound to an account or a session.
- The server cannot compute any slot because it does not know room keys or
  prekeys until they are published.
- A one-sided deposit has no effect and produces no signal to the recipient.
- Envelopes are opaque to the server and expire after 24 hours.

The full adversarial analysis lives in the threat model. The sections below
describe the mechanism as implemented.

## 5.2 Prekey bundle

Each user publishes a public prekey bundle that lets others seal envelopes to
them. It is stored server side and served by username.

```json
{
  "version": 1,
  "mlkem768Pk": "<hex 1184 bytes>",
  "ecdsaP256Pk": { "kty": "EC", "crv": "P-256", "x": "...", "y": "..." },
  "mldsa65Pk": "<hex 1952 bytes>",
  "sigEcdsa": "<b64url>",
  "sigMldsa": "<hex 3309 bytes>",
  "blockFilter": ["<hex64>"],
  "updatedAt": 1730000000000
}
```

The bundle is signed with both device keys over its canonical form, excluding
the two signature fields. Publication is idempotent; the server verifies both
signatures before storing. Rotation republishes the bundle with a new version
and new keys.

The fingerprint used throughout the protocol is:

```text
fp(pk) = SHA256(raw bytes of the ML-KEM-768 public key), lowercase hex 64 chars
```

## 5.3 Slots

A slot is a 32 byte opaque label computed only by clients. The epoch day binds a
slot to a 24 hour window.

```text
epochDay   = floor(now_ms / 86400000)

slotGlobal(recipientFp, day) =
    SHA256(UTF-8(recipientFp) || UTF-8(day))

slotContextual(recipientFp, roomKey, day) =
    SHA256(UTF-8(recipientFp) || UTF-8(SHA256(raw roomKey bytes)) || UTF-8(day))
```

The contextual slot proves that the depositor knows both the target prekey
fingerprint and the shared room key. The global slot proves only knowledge of
the prekey fingerprint. The server cannot compute either because it does not
know room keys and treats slots as opaque labels.

## 5.4 Envelope

An envelope has an outer layer visible to the server and an inner payload sealed
to the recipient.

### 5.4.1 Outer layer

```json
{
  "pv": 1,
  "slotId": "<hex64>",
  "recipientFp": "<hex64>",
  "senderHint": "<hex64>",
  "bucket": 16384,
  "ct": "<b64url>"
}
```

The server validates that `pv` is 1, that the three identifiers are exactly 64
hex characters, that the bucket is one of `4096`, `16384`, `65536`, and that
`ct` is non-empty and at most 96 KiB.

### 5.4.2 Inner layer

The `ct` field is `b64url(ML-KEM ciphertext || IV || AES-GCM ciphertext)`. After
decapsulation the recipient recovers this signed JSON:

```json
{
  "kind": "intro",
  "epochBucket": 19700,
  "sender": {
    "contextualPub": { "kty": "EC", "crv": "P-256", "x": "...", "y": "..." },
    "prekeyFp": "<hex64>",
    "mlkem768Pk": "<hex 1184 bytes>",
    "displayName": "alice"
  },
  "hybridSig": { "ecdsa": "<b64url>", "mldsa": "<hex 3309 bytes>" },
  "intro": "<text up to 2048 UTF-8 bytes>"
}
```

A `welcome` payload replaces `intro` with:

```json
{ "welcome": { "roomId": "<hex>", "roomKey": "<hex>" } }
```

### 5.4.3 Sealing

```text
innerPadded = pad(JSON(inner), bucket)          // 4 byte BE length prefix + JSON
sharedSecret, cipherText = ML-KEM-768.encapsulate(recipientMlkemPk)
aesKey = HKDF-SHA256(sharedSecret, empty, "qxphantom:v1") -> AES-256-GCM
iv = random 12 bytes
aead = AES-256-GCM(aesKey, iv, innerPadded)
ct = b64url(cipherText || iv || aead)
```

The inner JSON is signed with the hybrid signature before sealing. The padding
makes the outer size constant per bucket, so the server cannot infer the size of
the inner payload.

### 5.4.4 Opening

The recipient decapsulates with its ML-KEM secret key, derives the AES key,
decrypts, verifies the hybrid signature, verifies `epochBucket` against the
current day, and checks the sender against the local block list. Any failure is
silent.

## 5.5 Deposit and poll

Deposits and polls use anonymous HTTP and never carry an authorization header.

### 5.5.1 Deposit

`POST /api/phantom/deposit` accepts an envelope and a gate. The gate is:

```json
{
  "mode": "cap",
  "token": "<cap token or deposit token>",
  "nullifier": "<hex64>",
  "quotaToken": { "epoch": 0, "ticket": "...", "signature": "..." }
}
```

The server applies, in order:

1. A global rate limit.
2. Envelope and gate validation.
3. The opaque block check.
4. The anonymous quota nullifier check.
5. The mode-specific gate: `cap` verifies a CAPTCHA token, or `pass` consumes a
   Privacy Pass deposit token.

On success the envelope is stored in the RAM dead-drop. On any failure the
response is the same generic `{ "ok": false, "reason": "gate" }`.

### 5.5.2 Poll

`POST /api/phantom/poll` accepts up to 64 slots and a `want` value capped at 8.
The server claims at most one envelope per slot, FIFO, and pads the result to
`want` frames. Null frames are returned for missing envelopes. The server cannot
tell which frame the client can open because all frames are opaque to it.

### 5.5.3 Dead-drop store

The store is a RAM `HashMap<slotId, VecDeque<envelope>>` with these bounds:

| Limit | Value |
| --- | --- |
| Envelopes per slot | 16 |
| Total envelopes | 100 000 |
| Envelope TTL | 24 hours |
| Sweep interval | 60 seconds |

The store is never written to disk and is lost on restart.

## 5.6 Blocking

Blocking is two layered barriers.

**Server barrier (cost):** the server stores opaque tags only. For a block
update it computes:

```text
tag = SHA256(fp(owner prekey) || hint)
```

At deposit time it computes:

```text
tag = SHA256(recipientFp || senderHint)
```

and rejects if the tag exists. The server learns that a pair is blocked but not
which account blocks which account. The `blocks.user_id` column is used only for
the per-account quota of 512 and is never joined or exposed.

**Client barrier (guarantee):** the client keeps a local block list of sender
fingerprints and silently destroys any incoming envelope whose sender is in the
list, before any UI rendering.

## 5.7 Encrypted roster

The friend list is synchronized across devices through an opaque encrypted blob.
The blob key is:

```text
rosterKey = HKDF-SHA256(masterSecret, empty, "qxphantom:roster") -> AES-256-GCM
```

The blob is `b64(iv || ciphertext)`, at most 64 KiB. `PUT /api/social/blob`
enforces strict last-write-wins: the version must be strictly greater than the
current version, otherwise the server returns `409` with the current version.

The decrypted content is:

```json
{
  "friends": [],
  "pendingOut": [],
  "blocks": [],
  "settings": { "acceptUnknown": "off" }
}
```

## 5.8 Rendezvous state machine

```text
Shared-room path
A1 Alice computes slotContextual(Bob) using the shared room key
A2 Alice seals an intro and deposits it (cap gate)
A3 Alice polls her own slots on a jittered schedule
A4 Bob polls his rooms and global slots
A5 Bob opens the intro and decides
     ignore -> silence, envelope expires in 24 hours
     accept -> create room, seal a welcome, deposit it in Alice's global slot
A6 Alice receives the welcome and joins the room
A7 both store the friend locally and sync the roster

Username path
B1 Alice fetches Bob's prekey bundle
B2 Alice seals an intro and deposits it in slotGlobal(Bob)
B3 Bob, if acceptUnknown is enabled, opens and decides, then proceeds as A4 onward
```

Reciprocity is required for completion. There is no friendship without a second
deposit, the `welcome`, from the recipient. After completion only the two
clients know the edge.
