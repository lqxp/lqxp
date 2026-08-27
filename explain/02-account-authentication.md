# 02. Account Authentication

This chapter describes how a human account is created, authenticated, and
recovered. It covers the password and recovery-word path only. The separate
device and identity signature layer is in
[03-client-signature-protocol.md](./03-client-signature-protocol.md).

## 2.1 Credentials

An account has two independent secrets:

1. A **password** used for login and for destructive account actions.
2. A **recovery phrase** of 12 BIP39 words used only to reset the password.

Both secrets are stored as Argon2id hashes. The plaintext is never persisted by
the server.

```text
password         -> Argon2id -> users.password_hash
recovery phrase  -> Argon2id -> users.recovery_hash
```

## 2.2 Password and recovery hashing

The `core/security.rs` module owns secret handling.

| Function | Behavior |
| --- | --- |
| `hash_secret` | Argon2id (`Argon2::default()`) with a random salt. |
| `verify_secret` | Argon2id verification against a stored hash. |
| `verify_secret_constant_time` | Verifies against a fixed dummy hash when no real hash exists, then returns the same generic failure. This removes the timing signal that would otherwise reveal whether a username exists. |
| `token_hash` | `SHA256(session token)` as lowercase hex. |
| `generate_session_token` | 64 alphanumeric characters from the OS CSPRNG. |
| `generate_recovery_words` | A BIP39 mnemonic from 16 bytes of entropy, which produces 12 words. |

Username validation normalizes to lowercase and trimmed form, enforces a length
of 2 to 24 characters at registration, rejects a reserved `system` name, and
allows only a fixed whitelist of characters. Passwords must be 8 to 128
characters.

## 2.3 Registration

The `POST /api/auth/register` endpoint and the `services/auth.rs::register`
function create an account.

Steps:

1. Verify that registration is enabled in the feature flags.
2. Reject usernames that hit the operator blocklist.
3. Validate the username and password.
4. If the username is taken, burn a constant-time dummy hash and return a generic
   error.
5. Generate a random snowflake account id.
6. Generate 12 recovery words and hash them.
7. Hash the password.
8. Insert the user row.
9. Create a session token.

The response is:

```json
{
  "ok": true,
  "token": "<session token>",
  "user": { "...": "..." },
  "recoveryWords": ["word1", "word2", "..."]
}
```

The recovery words are returned exactly once, at registration. They are also
available for download from the client after a successful registration.

## 2.4 Login

The `POST /api/auth/login` endpoint and `services/auth.rs::login` function
authenticate a password.

Steps:

1. Normalize the username.
2. Look up the user.
3. Verify the password with Argon2id.
4. Reject banned or disabled accounts.
5. On a missing user, burn the constant-time dummy hash.
6. Create a session token.

The response is `{ "ok": true, "token": "...", "user": { ... } }`.

## 2.5 Session tokens

A session token is a 64 character alphanumeric string. The server stores only
`SHA256(token)` in the `sessions` table.

| Column | Meaning |
| --- | --- |
| `token_hash` | Primary key, the SHA-256 hex of the token. |
| `user_id` | The owning account. |
| `created_at` | Creation time in milliseconds. |
| `expires_at` | Expiry time in milliseconds. |

The session TTL is 7 days (`SESSION_TTL_MS`). `authenticate_token` resolves a
token to the user, rejecting expired tokens and disabled or banned accounts.
`touch_session` extends the TTL. `logout` deletes the session row. Recovery and
password changes invalidate all sessions for the account.

## 2.6 Recovery

The `POST /api/auth/recover` endpoint resets a password using the recovery
phrase.

Steps:

1. Normalize and validate the new password.
2. Look up the user.
3. Verify the normalized recovery phrase against `recovery_hash`.
4. Reject banned accounts.
5. Hash the new password and write it.
6. Delete all existing sessions for the account.
7. Create a fresh session token.

The recovery phrase is normalized the same way as at creation: split on
whitespace, lowercased, joined with single spaces.

## 2.7 WebSocket identification

The WebSocket protocol identifies a connection with opcode 2. The client sends
a token in either the `token` or `authToken` field. The server calls
`authenticate_token` and, on success, binds the session to a player record with
the account username, id, admin flag, badges, profile, and presence status.

The identification response includes the optional default room record:

```json
{
  "op": 2,
  "d": {
    "userId": "...",
    "username": "...",
    "admin": false,
    "profile": { "...": "..." },
    "status": "online",
    "badges": [],
    "defaultRoom": { "roomId": "...", "roomKey": "...", "title": "..." }
  }
}
```

## 2.8 HTTP bearer authentication

Authenticated HTTP endpoints use the `Authorization: Bearer <token>` header.
`authenticated_user` resolves the token through `authenticate_token`. The
endpoints that require a bearer token include the social blob, profile image
upload, username change, and the admin endpoints.

## 2.9 Account deletion

Deletion requires the current password. On success the server removes:

- All sessions for the account.
- The prekey bundle.
- The block tags.
- The user row.

It then disconnects all live sessions for the account, invalidates the public
profile cache, and removes any uploaded avatar and banner files.
