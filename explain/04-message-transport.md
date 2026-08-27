# 04. Message Transport

This chapter describes the real-time messaging transport, room lifecycle, and
the end-to-end encrypted message envelope.

## 4.1 WebSocket protocol

Messages on the WebSocket are JSON objects with an `op` field and a `d` data
field. The client sends operations; the server responds with the same opcode
where a reply is expected, or pushes events asynchronously.

```json
{ "op": 7, "d": { "text": "hello", "gameId": "<room id>" } }
```

The dispatch table from `websocket/protocol.rs::process_message` is:

| Opcode | Name | Direction |
| --- | --- | --- |
| 0 | Ping or error | Both |
| 1 | Heartbeat | Both |
| 2 | Identify (authenticate session) | Client to server |
| 3 | Join room | Both |
| 4 | Leave room | Client to server |
| 5 | Report kill | Client to server |
| 6 | Version query | Client to server |
| 7 | Send chat message | Both |
| 8 | Update client settings | Client to server |
| 18 | Request room history | Client to server |
| 19 | Toggle message reaction | Client to server |
| 21 | Delete message | Client to server |
| 28 | Request link preview | Client to server |
| 29 | Edit message | Client to server |
| 31 | Update typing state | Client to server |
| 32 | Upload room icon | Client to server |
| 33 | Update room title | Client to server |
| 35 | Request public profiles | Client to server |
| 36 | Publish prekey (PHANTOM) | Client to server |
| 37 | Fetch prekeys (PHANTOM) | Client to server |
| 39 | Update block list (PHANTOM) | Client to server |
| 40 | Create room | Client to server |
| 41 | Update room description | Client to server |
| 42 | Set member role | Client to server |
| 43 | Ban member | Client to server |
| 44 | Unban member | Client to server |
| 45 | Kick member | Client to server |
| 46 | Timeout member | Client to server |
| 47 | Transfer ownership | Client to server |
| 48 | Set chat lock | Client to server |
| 49 | Set moderator permissions | Client to server |
| 50 | Set calls enabled | Client to server |
| 51 | Set call access | Client to server |
| 52 | Unmute member | Client to server |
| 98 | Update voice chat state | Both |
| 100 | Update mute state | Client to server |
| 101 | Admin status | Client to server |
| 104 | Admin broadcast | Client to server |
| 105 | Stats query | Client to server |
| 110 | Update call media state | Both |
| 111 | Relay call signal | Both |
| 112 | Update call deafened state | Both |
| 23 | Link preview patch | Server to client |
| 999 | Session eviction | Server to client |

## 4.2 Room identity and access token

A room has two identifiers:

| Field | Bytes | Encoding |
| --- | --- | --- |
| `roomId` | 16 | lowercase hex, 32 characters |
| `roomKey` | 32 | lowercase hex, 64 characters |

A room access token is the concatenation `roomId || roomKey`, 96 hex
characters. The `roomId` is a routing label visible to the server; the `roomKey`
is the symmetric encryption secret shared by room members. The server stores
room metadata keyed by `roomId` but does not require the `roomKey` to route or
store messages.

## 4.3 Room lifecycle

Joining is opcode 3. The server:

1. Rate limits the join.
2. Validates the room id length and character set.
3. Requires an identified session.
4. Enforces a maximum of 100 rooms per player.
5. Rejects joins to a community room if the user is banned.
6. Persists the room record if it is new.
7. Broadcasts the updated roster to the room, unless the join is silent or the
   user is invisible.
8. Returns the roster, profiles, presence, role, icon, and room record to the
   joining client.
9. Dispatches the buffered room history.

Room membership lives in `PlayerSession.rooms`. Room metadata such as title,
icon, description, roles, bans, and timeouts is persisted in the `RoomDatabase`
backend.

Leaving is opcode 4. The server can also evict a session with opcode 999 when an
account is deleted or disabled.

## 4.4 Message sending

Messages are sent with opcode 7. A message is either plaintext or an encrypted
envelope, never both.

Validation performed by the server:

- The room id must be valid.
- If an encrypted envelope is present, its `roomId` must match the target room.
- An encrypted message must not include a plaintext `text` or `attachment`.
- The message must be non-empty.
- The sender must be identified and a member of the room.
- A minimum 400 millisecond interval applies between messages per session.
- A community room can restrict who may speak.
- Plaintext is truncated to 2000 characters.

The server constructs a `ChatMessageRecord`, stores it in the RAM message store,
and broadcasts it to all room members. The sender receives a separate
acknowledgment with the assigned message id.

## 4.5 Message storage

Room messages live in `AppState.room_messages`, a `HashMap<room_id,
Vec<ChatMessageRecord>>`. The store is capped at 150 messages per room; the
oldest are dropped when the cap is exceeded. The store is RAM only and is lost
on restart.

Room history is served on request with opcode 18, which streams the buffered
messages for the room to the requesting session.

## 4.6 End-to-end encrypted message envelope

Encrypted messages use a shared room key and a per-message derived AES-256-GCM
key. The format is version 2 with algorithm identifier `QXDR-A256GCM-HKDFSHA256`.

```json
{
  "v": 2,
  "alg": "QXDR-A256GCM-HKDFSHA256",
  "n": 1,
  "salt": "<b64url 32 bytes>",
  "iv": "<b64url 12 bytes>",
  "ciphertext": "<b64url>",
  "roomId": "<hex>",
  "senderDeviceId": "<hex 16 bytes>",
  "senderSigningKey": { "kty": "EC", "crv": "P-256", "x": "...", "y": "..." },
  "signature": "<b64url raw r||s>"
}
```

### 4.6.1 Key derivation

```text
messageKey = HKDF-SHA256(ikm  = roomKey bytes,
                         salt = salt,
                         info = "qxchat:e2ee:v2:" + roomId + ":" + n)
             -> AES-256-GCM key

aad = roomId + ":" + n + ":" + b64url(salt) + ":" + senderDeviceId
```

The plaintext JSON is encrypted with AES-256-GCM using the derived key, the
random 12 byte IV, and the AAD above.

### 4.6.2 Authentication

The envelope is signed with the sender device ECDSA P-256 key. The signed bytes
are the compact JSON of the following fields:

```text
{ v, alg, roomId, n, salt, iv, ciphertext, senderDeviceId, senderSigningKey }
```

The signature is the raw `r || s` form encoded as `b64url`. On receipt, the
client verifies the signature, optionally checks that the sender key matches a
trusted key, and then derives the message key and decrypts.

The server parses and bounds-check the envelope but cannot decrypt it because it
does not possess the room key.
