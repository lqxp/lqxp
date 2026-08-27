use std::{collections::BTreeMap, path::Path, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgPool, PgPoolOptions},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use tokio::fs;

use crate::core::{
    config::DatabaseConfig,
    models::{
        now_ms, status_from_str, status_to_str, ModeratorPermissions, RoomIcon, RoomKind, RoomRecord,
        RoomRole, UserPresenceStatus, UserProfile,
    },
    result::{ApiError, ApiResult},
    security::{
        generate_recovery_words, generate_session_token, generate_snowflake_id, hash_secret,
        normalize_recovery_phrase, normalize_username, token_hash, validate_password,
        validate_registration_username, validate_username, verify_secret, verify_secret_constant_time,
    },
};

const SESSION_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_USER_BADGES: usize = 16;
const MAX_USER_BADGE_LEN: usize = 32;
const MAX_BLOCKS_PER_ACCOUNT: usize = 512;
pub const MAX_SOCIAL_BLOB_BYTES: usize = 64 * 1024;

const USER_SELECT_BY_CREATED_DESC: &str =
    "SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users ORDER BY created_at DESC";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUser {
    pub id: String,
    pub username: String,
    pub profile: UserProfile,
    pub status: UserPresenceStatus,
    pub disabled: bool,
    pub banned: bool,
    pub admin: bool,
    pub badges: Vec<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: String,
    pub username: String,
    pub profile: UserProfile,
    pub status: UserPresenceStatus,
    pub disabled: bool,
    pub banned: bool,
    pub admin: bool,
    pub badges: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    #[serde(rename = "registerEnabled")]
    pub register_enabled: bool,
    #[serde(rename = "callsEnabled")]
    pub calls_enabled: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            register_enabled: true,
            calls_enabled: true,
        }
    }
}

/// Salon par défaut (« session par défaut ») : roomId + roomKey, configurable à
/// chaud par un admin. Le serveur détient volontairement la `room_key` E2EE du
/// salon officiel afin de pouvoir la redistribuer à chaque connexion (accepté
/// par conception pour un canal de news non sensible).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultRoom {
    pub room_id: String,
    pub room_key: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone)]
struct StoredUser {
    id: String,
    username: String,
    password_hash: String,
    recovery_hash: String,
    profile: UserProfile,
    status: UserPresenceStatus,
    disabled: bool,
    banned: bool,
    created_at: u64,
    user_rank: i64,
    username_changes: Vec<u64>,
    custom_badges: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct RawStoredUser {
    id: String,
    username: String,
    password_hash: String,
    recovery_hash: String,
    profile_json: String,
    status: String,
    disabled: i64,
    banned: i64,
    created_at: i64,
    user_rank: i64,
    username_changes_json: String,
    custom_badges_json: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredPrekey {
    pub bundle_json: String,
    pub updated_at: i64,
}

#[derive(Debug)]
enum SqlBackend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

#[derive(Debug)]
pub struct AccountDatabase {
    backend: SqlBackend,
    admin_ids: Vec<String>,
}

impl AccountDatabase {
    pub async fn connect(
        config: &DatabaseConfig,
        admin_ids: Vec<String>,
        register_enabled: bool,
    ) -> ApiResult<Self> {
        let kind = config.kind.trim().to_ascii_lowercase();
        let backend = if kind == "postgres" || kind == "postgresql" {
            SqlBackend::Postgres(
                PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&config.url)
                    .await
                    .map_err(|err| ApiError::internal("PostgreSQL connection", err))?,
            )
        } else {
            prepare_sqlite_path(&config.url).await?;
            let options = SqliteConnectOptions::from_str(&config.url)
                .map_err(|err| ApiError::internal("SQLite URL invalid", err))?
                .create_if_missing(true);
            SqlBackend::Sqlite(
                SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(options)
                    .await
                    .map_err(|err| ApiError::internal("SQLite connection", err))?,
            )
        };

        let db = Self { backend, admin_ids };
        db.migrate().await?;
        db.ensure_feature_defaults(register_enabled).await?;
        Ok(db)
    }

    async fn migrate(&self) -> ApiResult<()> {
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                recovery_hash TEXT NOT NULL,
                profile_json TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'online',
                disabled BIGINT NOT NULL DEFAULT 0,
                banned BIGINT NOT NULL DEFAULT 0,
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL,
                username_changes_json TEXT NOT NULL DEFAULT '[]',
                custom_badges_json TEXT NOT NULL DEFAULT '[]'
            )
            "#,
        )
        .await?;
        self.ensure_column("users", "banned", "banned BIGINT NOT NULL DEFAULT 0")
            .await?;
        self.ensure_column(
            "users",
            "custom_badges_json",
            "custom_badges_json TEXT NOT NULL DEFAULT '[]'",
        )
        .await?;
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                token_hash TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                expires_at BIGINT NOT NULL
            )
            "#,
        )
        .await?;
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS feature_flags (
                key TEXT PRIMARY KEY,
                enabled BIGINT NOT NULL
            )
            "#,
        )
        .await?;

        // QXP-PHANTOM : préclés publiques, tags de blocage opaques, blob roster.
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS prekeys (
                user_id TEXT PRIMARY KEY,
                bundle_json TEXT NOT NULL,
                updated_at BIGINT NOT NULL
            )
            "#,
        )
        .await?;
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS blocks (
                tag TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                created_at BIGINT NOT NULL
            )
            "#,
        )
        .await?;
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS default_room (
                id INTEGER PRIMARY KEY,
                room_id TEXT NOT NULL,
                room_key TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                updated_at BIGINT NOT NULL
            )
            "#,
        )
        .await?;
        self.ensure_column("users", "social_blob", "social_blob TEXT NULL")
            .await?;
        self.ensure_column(
            "users",
            "social_blob_ver",
            "social_blob_ver BIGINT NOT NULL DEFAULT 0",
        )
        .await?;
        Ok(())
    }

    async fn execute(&self, sql: &str) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Database execution", err))
    }

    async fn ensure_column(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> ApiResult<()> {
        let exists = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("Database schema query", err))?;
                rows.iter().any(|row| {
                    row.try_get::<String, _>("name")
                        .map(|name| name == column)
                        .unwrap_or(false)
                })
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = $2",
                )
                .bind(table)
                .bind(column)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Database schema query", err))?;
                row.is_some()
            }
        };
        if exists {
            return Ok(());
        }
        self.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
            .await
    }

    async fn ensure_feature_defaults(&self, register_enabled: bool) -> ApiResult<()> {
        self.set_feature_default("register_enabled", register_enabled).await?;
        self.set_feature_default("calls_enabled", true).await?;
        Ok(())
    }

    async fn set_feature_default(&self, key: &str, enabled: bool) -> ApiResult<()> {
        let exists = self.feature_value(key).await?.is_some();
        if exists {
            return Ok(());
        }
        self.set_feature(key, enabled).await
    }

    pub async fn feature_flags(&self) -> ApiResult<FeatureFlags> {
        Ok(FeatureFlags {
            register_enabled: self
                .feature_value("register_enabled")
                .await?
                .unwrap_or(true),
            calls_enabled: self.feature_value("calls_enabled").await?.unwrap_or(true),
        })
    }

    async fn feature_value(&self, key: &str) -> ApiResult<Option<bool>> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT enabled FROM feature_flags WHERE key = ?")
                    .bind(key)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Database feature query", err))?;
                Ok(row.map(|row| row.get::<i64, _>("enabled") != 0))
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT enabled FROM feature_flags WHERE key = $1")
                    .bind(key)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Database feature query", err))?;
                Ok(row.map(|row| row.get::<i64, _>("enabled") != 0))
            }
        }
    }

    pub async fn set_feature(&self, key: &str, enabled: bool) -> ApiResult<()> {
        let value = if enabled { 1i64 } else { 0i64 };
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(
                "INSERT INTO feature_flags (key, enabled) VALUES (?, ?) \
                     ON CONFLICT(key) DO UPDATE SET enabled = excluded.enabled",
            )
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query(
                "INSERT INTO feature_flags (key, enabled) VALUES ($1, $2) \
                     ON CONFLICT(key) DO UPDATE SET enabled = excluded.enabled",
            )
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Database set feature", err))
    }

    pub async fn set_default_room(
        &self,
        room_id: &str,
        room_key: &str,
        title: &str,
    ) -> ApiResult<()> {
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(
                "INSERT INTO default_room (id, room_id, room_key, title, updated_at) VALUES (1, ?, ?, ?, ?) \
                     ON CONFLICT(id) DO UPDATE SET room_id = excluded.room_id, room_key = excluded.room_key, title = excluded.title, updated_at = excluded.updated_at",
            )
            .bind(room_id)
            .bind(room_key)
            .bind(title)
            .bind(now)
            .execute(pool)
            .await
            .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query(
                "INSERT INTO default_room (id, room_id, room_key, title, updated_at) VALUES (1, $1, $2, $3, $4) \
                     ON CONFLICT (id) DO UPDATE SET room_id = EXCLUDED.room_id, room_key = EXCLUDED.room_key, title = EXCLUDED.title, updated_at = EXCLUDED.updated_at",
            )
            .bind(room_id)
            .bind(room_key)
            .bind(title)
            .bind(now)
            .execute(pool)
            .await
            .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Database set default room", err))
    }

    pub async fn get_default_room(&self) -> ApiResult<Option<DefaultRoom>> {
        let room = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("SELECT room_id, room_key, title FROM default_room WHERE id = 1")
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Database get default room", err))?
                    .map(|row| DefaultRoom {
                        room_id: row.get::<String, _>("room_id"),
                        room_key: row.get::<String, _>("room_key"),
                        title: row.get::<String, _>("title"),
                    })
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("SELECT room_id, room_key, title FROM default_room WHERE id = 1")
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Database get default room", err))?
                    .map(|row| DefaultRoom {
                        room_id: row.get::<String, _>("room_id"),
                        room_key: row.get::<String, _>("room_key"),
                        title: row.get::<String, _>("title"),
                    })
            }
        };
        Ok(room)
    }

    pub async fn clear_default_room(&self) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM default_room WHERE id = 1")
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM default_room WHERE id = 1")
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Database clear default room", err))
    }

    pub async fn register(
        &self,
        username: &str,
        password: &str,
    ) -> ApiResult<(PublicUser, String, Vec<String>)> {
        let username = validate_registration_username(username)?;
        validate_password(password)?;
        if self.user_by_username(&username).await?.is_some() {
            let _ = verify_secret_constant_time(password, None);
            return Err(ApiError::bad_request("Registration request could not be processed."));
        }

        let id = generate_snowflake_id();
        let recovery_words = generate_recovery_words();
        let recovery_phrase = recovery_words.join(" ");
        let password_hash = hash_secret(password)?;
        let recovery_hash = hash_secret(&recovery_phrase)?;
        let now = now_ms();
        let profile_json = serde_json::to_string(&UserProfile::default())
            .map_err(|err| ApiError::internal("Profile encoding", err))?;

        let result = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO users \
                     (id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, updated_at, username_changes_json, custom_badges_json) \
                     VALUES (?, ?, ?, ?, ?, 'online', 0, 0, ?, ?, '[]', '[]')",
                )
                .bind(&id)
                .bind(&username)
                .bind(&password_hash)
                .bind(&recovery_hash)
                .bind(&profile_json)
                .bind(now as i64)
                .bind(now as i64)
                .execute(pool)
                .await
                .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO users \
                     (id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, updated_at, username_changes_json, custom_badges_json) \
                     VALUES ($1, $2, $3, $4, $5, 'online', 0, 0, $6, $7, '[]', '[]')",
                )
                .bind(&id)
                .bind(&username)
                .bind(&password_hash)
                .bind(&recovery_hash)
                .bind(&profile_json)
                .bind(now as i64)
                .bind(now as i64)
                .execute(pool)
                .await
                .map(|_| ())
            }
        };
        if result.is_err() {
            return Err(ApiError::bad_request("Registration request could not be processed."));
        }

        let user = self
            .user_by_id(&id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account was not created."))?;
        let token = self.create_session(&id).await?;
        Ok((self.public_user(user), token, recovery_words))
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> ApiResult<(PublicUser, String)> {
        let username = normalize_username(username);
        let user_opt = self.user_by_username(&username).await?;

        let user = match user_opt {
            Some(user) => {
                verify_secret(password, &user.password_hash)?;
                if user.banned {
                    return Err(ApiError::forbidden("Account is banned."));
                }
                if user.disabled {
                    return Err(ApiError::forbidden("Account is disabled."));
                }
                user
            }
            None => {
                let _ = verify_secret_constant_time(password, None);
                return Err(ApiError::unauthorized("Invalid credentials."));
            }
        };

        let token = self.create_session(&user.id).await?;
        Ok((self.public_user(user), token))
    }

    pub async fn recover(
        &self,
        username: &str,
        recovery_words: &str,
        new_password: &str,
    ) -> ApiResult<(PublicUser, String)> {
        validate_password(new_password)?;
        let username = normalize_username(username);
        let user_opt = self.user_by_username(&username).await?;

        let user = match user_opt {
            Some(user) => {
                verify_secret(
                    &normalize_recovery_phrase(recovery_words),
                    &user.recovery_hash,
                )?;
                if user.banned {
                    return Err(ApiError::forbidden("Account is banned."));
                }
                user
            }
            None => {
                let _ = verify_secret_constant_time(&normalize_recovery_phrase(recovery_words), None);
                return Err(ApiError::bad_request("Invalid recovery credentials."));
            }
        };

        let password_hash = hash_secret(new_password)?;
        let now = now_ms();
        self.update_password_hash(&user.id, &password_hash, now)
            .await?;
        self.invalidate_user_sessions(&user.id).await?;
        let token = self.create_session(&user.id).await?;
        let updated = self
            .user_by_id(&user.id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account not found."))?;
        Ok((self.public_user(updated), token))
    }

    pub async fn invalidate_user_sessions(&self, user_id: &str) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Session invalidation", err))
    }

    pub async fn create_session(&self, user_id: &str) -> ApiResult<String> {
        let token = generate_session_token();
        let hash = token_hash(&token);
        let now = now_ms() as i64;
        let expires_at = (now_ms() + SESSION_TTL_MS) as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
                    .bind(hash)
                    .bind(user_id)
                    .bind(now)
                    .bind(expires_at)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)")
                    .bind(hash)
                    .bind(user_id)
                    .bind(now)
                    .bind(expires_at)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Session creation", err))?;
        Ok(token)
    }

    pub async fn authenticate_token(
        &self,
        token: &str,
    ) -> ApiResult<Option<AuthenticatedUser>> {
        let hash = token_hash(token);
        let now = now_ms() as i64;
        let user_id = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query(
                    "SELECT user_id FROM sessions WHERE token_hash = ? AND expires_at > ?",
                )
                .bind(&hash)
                .bind(now)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Session authentication", err))?;
                row.map(|row| row.get::<String, _>("user_id"))
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > $2",
                )
                .bind(&hash)
                .bind(now)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Session authentication", err))?;
                row.map(|row| row.get::<String, _>("user_id"))
            }
        };
        let Some(user_id) = user_id else {
            return Ok(None);
        };
        let Some(user) = self.user_by_id(&user_id).await? else {
            return Ok(None);
        };
        if user.disabled || user.banned {
            return Ok(None);
        }
        let badges = self.user_badges(&user);
        Ok(Some(AuthenticatedUser {
            id: user.id.clone(),
            username: user.username.clone(),
            profile: user.profile.clone(),
            status: user.status,
            disabled: user.disabled,
            banned: user.banned,
            admin: self.is_admin(&user.id),
            badges,
        }))
    }

    pub async fn touch_session(&self, token: &str) -> ApiResult<()> {
        let hash = token_hash(token);
        let expires_at = (now_ms() + SESSION_TTL_MS) as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("UPDATE sessions SET expires_at = ? WHERE token_hash = ?")
                .bind(expires_at)
                .bind(hash)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("UPDATE sessions SET expires_at = $1 WHERE token_hash = $2")
                .bind(expires_at)
                .bind(hash)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Session update", err))
    }

    pub async fn logout(&self, token: &str) -> ApiResult<()> {
        let hash = token_hash(token);
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
                .bind(hash)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
                .bind(hash)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Session deletion", err))
    }

    pub async fn delete_account(&self, token: &str, password: &str) -> ApiResult<()> {
        let Some(user) = self.authenticate_token(token).await? else {
            return Err(ApiError::unauthorized("Not authenticated."));
        };
        let stored = self
            .user_by_id(&user.id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account not found."))?;
        verify_secret(password, &stored.password_hash)?;
        self.delete_user_account(&user.id).await
    }

    pub async fn delete_user_account(&self, user_id: &str) -> ApiResult<()> {
        if self.user_by_id(user_id).await?.is_none() {
            return Err(ApiError::bad_request("Account not found."));
        }
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user sessions", err))?;
                sqlx::query("DELETE FROM prekeys WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user prekeys", err))?;
                sqlx::query("DELETE FROM blocks WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user blocks", err))?;
                sqlx::query("DELETE FROM users WHERE id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user record", err))
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user sessions", err))?;
                sqlx::query("DELETE FROM prekeys WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user prekeys", err))?;
                sqlx::query("DELETE FROM blocks WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user blocks", err))?;
                sqlx::query("DELETE FROM users WHERE id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| ApiError::internal("Delete user record", err))
            }
        }
    }

    pub async fn publish_prekey(&self, user_id: &str, bundle_json: &str) -> ApiResult<()> {
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO prekeys (user_id, bundle_json, updated_at) VALUES (?, ?, ?) \
                     ON CONFLICT(user_id) DO UPDATE SET bundle_json = excluded.bundle_json, updated_at = excluded.updated_at",
                )
                .bind(user_id)
                .bind(bundle_json)
                .bind(now)
                .execute(pool)
                .await
                .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO prekeys (user_id, bundle_json, updated_at) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id) DO UPDATE SET bundle_json = EXCLUDED.bundle_json, updated_at = EXCLUDED.updated_at",
                )
                .bind(user_id)
                .bind(bundle_json)
                .bind(now)
                .execute(pool)
                .await
                .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Publish prekey", err))
    }

    pub async fn get_prekey_by_user_id(&self, user_id: &str) -> ApiResult<Option<StoredPrekey>> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, StoredPrekey>(
                    "SELECT bundle_json, updated_at FROM prekeys WHERE user_id = ?",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, StoredPrekey>(
                    "SELECT bundle_json, updated_at FROM prekeys WHERE user_id = $1",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
            }
        }
        .map_err(|err| ApiError::internal("Fetch prekey", err))
    }

    pub async fn get_prekey_by_username(&self, username: &str) -> ApiResult<Option<StoredPrekey>> {
        let Some(user) = self.user_by_username(username).await? else {
            return Ok(None);
        };
        self.get_prekey_by_user_id(&user.id).await
    }

    pub async fn add_block_tag(&self, user_id: &str, tag: &str) -> ApiResult<bool> {
        if self.count_block_tags(user_id).await? >= MAX_BLOCKS_PER_ACCOUNT {
            return Ok(false);
        }
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT OR IGNORE INTO blocks (tag, user_id, created_at) VALUES (?, ?, ?)",
                )
                .bind(tag)
                .bind(user_id)
                .bind(now)
                .execute(pool)
                .await
                .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO blocks (tag, user_id, created_at) VALUES ($1, $2, $3) ON CONFLICT (tag) DO NOTHING",
                )
                .bind(tag)
                .bind(user_id)
                .bind(now)
                .execute(pool)
                .await
                .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Add block tag", err))?;
        Ok(true)
    }

    pub async fn remove_block_tag(&self, user_id: &str, tag: &str) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM blocks WHERE tag = ? AND user_id = ?")
                    .bind(tag)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM blocks WHERE tag = $1 AND user_id = $2")
                    .bind(tag)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Remove block tag", err))
    }

    pub async fn count_block_tags(&self, user_id: &str) -> ApiResult<usize> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT COUNT(*) FROM blocks WHERE user_id = ?")
                    .bind(user_id)
                    .fetch_one(pool)
                    .await
                    .map_err(|err| ApiError::internal("Count block tags", err))?;
                Ok(row.get::<i64, _>(0) as usize)
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT COUNT(*) FROM blocks WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(pool)
                    .await
                    .map_err(|err| ApiError::internal("Count block tags", err))?;
                Ok(row.get::<i64, _>(0) as usize)
            }
        }
    }

    pub async fn list_block_tags(&self, user_id: &str) -> ApiResult<Vec<String>> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let rows = sqlx::query("SELECT tag FROM blocks WHERE user_id = ?")
                    .bind(user_id)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("List block tags", err))?;
                Ok(rows.iter().map(|row| row.get::<String, _>("tag")).collect())
            }
            SqlBackend::Postgres(pool) => {
                let rows = sqlx::query("SELECT tag FROM blocks WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("List block tags", err))?;
                Ok(rows.iter().map(|row| row.get::<String, _>("tag")).collect())
            }
        }
    }

    pub async fn is_blocked_tag(&self, tag: &str) -> ApiResult<bool> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT 1 FROM blocks WHERE tag = ?")
                    .bind(tag)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Check block tag", err))?;
                Ok(row.is_some())
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT 1 FROM blocks WHERE tag = $1")
                    .bind(tag)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| ApiError::internal("Check block tag", err))?;
                Ok(row.is_some())
            }
        }
    }

    pub async fn get_social_blob(&self, user_id: &str) -> ApiResult<(i64, Option<String>)> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query(
                    "SELECT social_blob_ver, social_blob FROM users WHERE id = ?",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Fetch social blob", err))?;
                match row {
                    Some(row) => Ok((
                        row.get::<i64, _>("social_blob_ver"),
                        row.get::<Option<String>, _>("social_blob"),
                    )),
                    None => Ok((0, None)),
                }
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT social_blob_ver, social_blob FROM users WHERE id = $1",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Fetch social blob", err))?;
                match row {
                    Some(row) => Ok((
                        row.get::<i64, _>("social_blob_ver"),
                        row.get::<Option<String>, _>("social_blob"),
                    )),
                    None => Ok((0, None)),
                }
            }
        }
    }

    /// Retourne `Ok(Some(ver_courant))` en cas de conflit LWW, `Ok(None)` sinon.
    pub async fn put_social_blob(&self, user_id: &str, ver: i64, blob: &str) -> ApiResult<Option<i64>> {
        let (current_ver, _) = self.get_social_blob(user_id).await?;
        if ver <= current_ver {
            return Ok(Some(current_ver));
        }
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET social_blob = ?, social_blob_ver = ? WHERE id = ?")
                    .bind(blob)
                    .bind(ver)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET social_blob = $1, social_blob_ver = $2 WHERE id = $3")
                    .bind(blob)
                    .bind(ver)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Update social blob", err))?;
        Ok(None)
    }

    pub async fn purge_accounts(
        &self,
        created_after_ms: Option<u64>,
        created_before_ms: Option<u64>,
        min_username_len: Option<usize>,
        max_username_len: Option<usize>,
        username_contains: Option<&str>,
        exclude_admin: bool,
    ) -> ApiResult<usize> {
        let users = self.list_users().await?;
        let mut count = 0usize;
        for user in users {
            if exclude_admin && user.admin {
                continue;
            }
            if let Some(after) = created_after_ms {
                if user.created_at < after {
                    continue;
                }
            }
            if let Some(before) = created_before_ms {
                if user.created_at > before {
                    continue;
                }
            }
            let char_len = user.username.chars().count();
            if let Some(min_len) = min_username_len {
                if char_len < min_len {
                    continue;
                }
            }
            if let Some(max_len) = max_username_len {
                if char_len > max_len {
                    continue;
                }
            }
            if let Some(pat) = username_contains {
                if !pat.is_empty() && !user.username.to_lowercase().contains(&pat.to_lowercase()) {
                    continue;
                }
            }
            let _ = self.delete_user_account(&user.id).await;
            count += 1;
        }
        Ok(count)
    }

    pub async fn me(&self, token: &str) -> ApiResult<Option<(PublicUser, String)>> {
        let Some(user) = self.authenticate_token(token).await? else {
            return Ok(None);
        };
        self.touch_session(token).await?;
        Ok(Some((
            PublicUser {
                id: user.id.clone(),
                username: user.username.clone(),
                profile: user.profile.clone(),
                status: user.status,
                disabled: user.disabled,
                banned: user.banned,
                admin: user.admin,
                badges: user.badges,
                created_at: 0,
            },
            token.to_owned(),
        )))
    }

    pub async fn profiles_by_usernames(
        &self,
        usernames: &[String],
    ) -> ApiResult<Vec<(String, UserProfile)>> {
        let mut profiles = Vec::new();
        for username in usernames {
            if let Some(user) = self.user_by_username(username).await? {
                profiles.push((user.username, user.profile));
            }
        }
        Ok(profiles)
    }

    pub async fn public_user_by_id_or_username(
        &self,
        user_id: Option<&str>,
        username: Option<&str>,
    ) -> ApiResult<Option<PublicUser>> {
        if let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(user) = self.user_by_id(user_id).await? {
                return Ok(Some(self.public_user(user)));
            }
        }

        if let Some(username) = username.map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(user) = self.user_by_username(username).await? {
                return Ok(Some(self.public_user(user)));
            }
        }

        Ok(None)
    }

    pub async fn profile_uses_file_id(&self, file_id: &str) -> ApiResult<bool> {
        let pattern = format!("%\"id\":\"{}\"%", file_id.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let found = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("SELECT 1 FROM users WHERE profile_json LIKE ? ESCAPE '\\' LIMIT 1")
                .bind(&pattern)
                .fetch_optional(pool)
                .await
                .map(|row| row.is_some()),
            SqlBackend::Postgres(pool) => sqlx::query("SELECT 1 FROM users WHERE profile_json LIKE $1 ESCAPE '\\' LIMIT 1")
                .bind(&pattern)
                .fetch_optional(pool)
                .await
                .map(|row| row.is_some()),
        }
        .map_err(|err| ApiError::internal("Profile file search", err))?;
        Ok(found)
    }

    pub async fn update_profile(&self, user_id: &str, profile: &UserProfile) -> ApiResult<()> {
        let profile_json = serde_json::to_string(profile)
            .map_err(|err| ApiError::internal("Profile encoding", err))?;
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET profile_json = ?, updated_at = ? WHERE id = ?")
                    .bind(profile_json)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET profile_json = $1, updated_at = $2 WHERE id = $3")
                    .bind(profile_json)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Profile update", err))
    }

    pub async fn update_status(
        &self,
        user_id: &str,
        status: UserPresenceStatus,
    ) -> ApiResult<()> {
        let status_text = status_to_str(status);
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(status_text)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET status = $1, updated_at = $2 WHERE id = $3")
                    .bind(status_text)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Status update", err))
    }

    pub async fn change_username(
        &self,
        user_id: &str,
        username: &str,
    ) -> ApiResult<PublicUser> {
        let username = validate_username(username)?;
        let mut user = self
            .user_by_id(user_id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account not found."))?;

        if user.username == username {
            return Ok(self.public_user(user));
        }

        let now = now_ms();
        let window_start = now.saturating_sub(7 * 24 * 60 * 60 * 1000);
        user.username_changes.retain(|stamp| *stamp >= window_start);
        if !user.username_changes.is_empty() {
            return Err(ApiError::too_many_requests(
                "Username can only be changed once per week.",
            ));
        }

        if let Some(existing) = self.user_by_username(&username).await? {
            if existing.id != user_id {
                let _ = verify_secret_constant_time("dummy_check_prevent_timing", None);
                return Err(ApiError::bad_request("Username is not available."));
            }
        }

        user.username_changes.push(now);
        let changes_json = serde_json::to_string(&user.username_changes)
            .map_err(|err| ApiError::internal("Username changes encoding", err))?;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("UPDATE users SET username = ?, username_changes_json = ?, updated_at = ? WHERE id = ?")
                .bind(&username)
                .bind(changes_json)
                .bind(now as i64)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("UPDATE users SET username = $1, username_changes_json = $2, updated_at = $3 WHERE id = $4")
                .bind(&username)
                .bind(changes_json)
                .bind(now as i64)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Username update", err))?;
        let updated = self
            .user_by_id(user_id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account not found."))?;
        Ok(self.public_user(updated))
    }

    pub async fn list_users(&self) -> ApiResult<Vec<PublicUser>> {
        let rows = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_CREATED_DESC)
                    .fetch_all(pool)
                    .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_CREATED_DESC)
                    .fetch_all(pool)
                    .await
            }
        }
        .map_err(|err| ApiError::internal("List users query", err))?;
        rows.into_iter()
            .map(|row| self.stored_from_raw(row).map(|user| self.public_user(user)))
            .collect()
    }

    pub async fn count_users(&self) -> ApiResult<u64> {
        let count = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                .fetch_one(pool)
                .await,
            SqlBackend::Postgres(pool) => {
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                    .fetch_one(pool)
                    .await
            }
        }
        .map_err(|err| ApiError::internal("Count users query", err))?;
        Ok(count.max(0) as u64)
    }

    /// Admin username search: never loads the whole table. Bounded SQL
    /// candidates (exact → prefix → substring) are then ranked in Rust by
    /// edit distance, so typos still surface and only the top `limit`
    /// matches are returned.
    pub async fn search_users(&self, query: &str, limit: usize) -> ApiResult<Vec<PublicUser>> {
        // Usernames are capped at 32 chars server-side; bound the needle so
        // huge queries can't turn into expensive LIKE scans.
        let needle: String = query.trim().to_lowercase().chars().take(64).collect();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 50);
        // Ask the DB for a few extra candidates so the in-Rust ranking has
        // room to promote close matches.
        let sql_limit = (limit * 4).max(100) as i64;
        let prefix = format!("{}%", escape_like_pattern(&needle));
        let substring = format!("%{}%", escape_like_pattern(&needle));

        let select = "SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users";
        let rows = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let sql = format!(
                    "{select} WHERE LOWER(username) = ? OR LOWER(username) LIKE ? ESCAPE '\\' OR LOWER(username) LIKE ? ESCAPE '\\' ORDER BY CASE WHEN LOWER(username) = ? THEN 0 WHEN LOWER(username) LIKE ? ESCAPE '\\' THEN 1 ELSE 2 END ASC, created_at DESC LIMIT ?"
                );
                sqlx::query_as::<_, RawStoredUser>(&sql)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(&substring)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(sql_limit)
                    .fetch_all(pool)
                    .await
            }
            SqlBackend::Postgres(pool) => {
                let sql = format!(
                    "{select} WHERE LOWER(username) = $1 OR LOWER(username) LIKE $2 ESCAPE '\\' OR LOWER(username) LIKE $3 ESCAPE '\\' ORDER BY CASE WHEN LOWER(username) = $1 THEN 0 WHEN LOWER(username) LIKE $2 ESCAPE '\\' THEN 1 ELSE 2 END ASC, created_at DESC LIMIT $4"
                );
                sqlx::query_as::<_, RawStoredUser>(&sql)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(&substring)
                    .bind(sql_limit)
                    .fetch_all(pool)
                    .await
            }
        }
        .map_err(|err| ApiError::internal("Search users query", err))?;

        let mut ranked: Vec<(u8, usize, StoredUser)> = Vec::with_capacity(rows.len());
        for row in rows {
            let user = self.stored_from_raw(row)?;
            let username = user.username.to_lowercase();
            let class = if username == needle {
                0
            } else if username.starts_with(&needle) {
                1
            } else {
                2
            };
            ranked.push((class, levenshtein_distance(&username, &needle), user));
        }

        // Exact matches first, then prefix, then substring; within a class
        // the closest (smallest edit distance) and most recent come first.
        ranked.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                .then(b.2.created_at.cmp(&a.2.created_at))
        });
        ranked.truncate(limit);
        Ok(ranked
            .into_iter()
            .map(|(_, _, user)| self.public_user(user))
            .collect())
    }

    pub async fn set_user_disabled(&self, user_id: &str, disabled: bool) -> ApiResult<()> {
        let value = if disabled { 1i64 } else { 0i64 };
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("UPDATE users SET disabled = ?, updated_at = ? WHERE id = ?")
                .bind(value)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("UPDATE users SET disabled = $1, updated_at = $2 WHERE id = $3")
                .bind(value)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Set user disabled", err))
    }

    pub async fn set_user_banned(&self, user_id: &str, banned: bool) -> ApiResult<()> {
        let value = if banned { 1i64 } else { 0i64 };
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("UPDATE users SET banned = ?, updated_at = ? WHERE id = ?")
                .bind(value)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("UPDATE users SET banned = $1, updated_at = $2 WHERE id = $3")
                .bind(value)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Set user banned", err))
    }

    pub async fn set_user_badges(
        &self,
        user_id: &str,
        badges: &[String],
    ) -> ApiResult<PublicUser> {
        let sanitized = sanitize_custom_badges(badges);
        let badges_json = serde_json::to_string(&sanitized)
            .map_err(|err| ApiError::internal("Badges encoding", err))?;
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("UPDATE users SET custom_badges_json = ?, updated_at = ? WHERE id = ?")
                .bind(badges_json)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("UPDATE users SET custom_badges_json = $1, updated_at = $2 WHERE id = $3")
                .bind(badges_json)
                .bind(now)
                .bind(user_id)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Set user badges", err))?;
        let updated = self
            .user_by_id(user_id)
            .await?
            .ok_or_else(|| ApiError::bad_request("Account not found."))?;
        Ok(self.public_user(updated))
    }

    async fn user_by_id(&self, user_id: &str) -> ApiResult<Option<StoredUser>> {
        let raw = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, RawStoredUser>(
                    "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) WHERE id = ?",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, RawStoredUser>(
                    "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users WHERE id = $1",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
            }
        }
        .map_err(|err| ApiError::internal("User lookup by ID", err))?;

        raw.map(|row| self.stored_from_raw(row)).transpose()
    }

    async fn user_by_username(&self, username: &str) -> ApiResult<Option<StoredUser>> {
        let normalized = normalize_username(username);
        let raw = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, RawStoredUser>(
                    "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) WHERE username = ?",
                )
                .bind(&normalized)
                .fetch_optional(pool)
                .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, RawStoredUser>(
                    "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users WHERE username = $1",
                )
                .bind(&normalized)
                .fetch_optional(pool)
                .await
            }
        }
        .map_err(|err| ApiError::internal("User lookup by username", err))?;

        raw.map(|row| self.stored_from_raw(row)).transpose()
    }

    async fn update_password_hash(
        &self,
        user_id: &str,
        password_hash: &str,
        now: u64,
    ) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
                    .bind(password_hash)
                    .bind(now as i64)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET password_hash = $1, updated_at = $2 WHERE id = $3")
                    .bind(password_hash)
                    .bind(now as i64)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| ApiError::internal("Update password hash", err))
    }

    fn stored_from_raw(&self, raw: RawStoredUser) -> ApiResult<StoredUser> {
        let profile = serde_json::from_str::<UserProfile>(&raw.profile_json)
            .unwrap_or_default();
        let username_changes = serde_json::from_str::<Vec<u64>>(&raw.username_changes_json)
            .unwrap_or_default();
        let custom_badges = serde_json::from_str::<Vec<String>>(&raw.custom_badges_json)
            .unwrap_or_default();
        Ok(StoredUser {
            id: raw.id,
            username: raw.username,
            password_hash: raw.password_hash,
            recovery_hash: raw.recovery_hash,
            profile,
            status: status_from_str(&raw.status),
            disabled: raw.disabled != 0,
            banned: raw.banned != 0,
            created_at: raw.created_at as u64,
            user_rank: raw.user_rank,
            username_changes,
            custom_badges,
        })
    }

    fn is_admin(&self, user_id: &str) -> bool {
        self.admin_ids.iter().any(|id| id == user_id)
    }

    fn user_badges(&self, user: &StoredUser) -> Vec<String> {
        let mut badges = Vec::new();
        if self.is_admin(&user.id) {
            badges.push("admin".to_owned());
        }
        if user.user_rank > 0 && user.user_rank <= 200 {
            badges.push("early".to_owned());
        }
        for badge in &user.custom_badges {
            if !badges.contains(badge) {
                badges.push(badge.clone());
            }
        }
        badges
    }

    fn public_user(&self, user: StoredUser) -> PublicUser {
        let badges = self.user_badges(&user);
        let is_admin = self.is_admin(&user.id);
        PublicUser {
            id: user.id,
            username: user.username,
            profile: user.profile,
            status: user.status,
            disabled: user.disabled,
            banned: user.banned,
            admin: is_admin,
            badges,
            created_at: user.created_at,
        }
    }
}

fn sanitize_custom_badges(badges: &[String]) -> Vec<String> {
    let mut cleaned = Vec::new();
    for badge in badges {
        let trimmed = badge.trim().to_lowercase();
        if trimmed.is_empty() || trimmed == "admin" {
            continue;
        }
        let sanitized: String = trimmed
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
            .take(MAX_USER_BADGE_LEN)
            .collect();
        if !sanitized.is_empty() && !cleaned.contains(&sanitized) {
            cleaned.push(sanitized);
            if cleaned.len() >= MAX_USER_BADGES {
                break;
            }
        }
    }
    cleaned
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[derive(Debug)]
pub struct RoomDatabase {
    backend: SqlBackend,
}

impl RoomDatabase {
    pub async fn connect(config: &DatabaseConfig) -> ApiResult<Self> {
        let kind = config.kind.trim().to_ascii_lowercase();
        let backend = if kind == "postgres" || kind == "postgresql" {
            SqlBackend::Postgres(
                PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&config.url)
                    .await
                    .map_err(|err| ApiError::internal("PostgreSQL connection", err))?,
            )
        } else {
            prepare_sqlite_path(&config.url).await?;
            let options = SqliteConnectOptions::from_str(&config.url)
                .map_err(|err| ApiError::internal("Invalid SQLite room database URL", err))?
                .create_if_missing(true);
            SqlBackend::Sqlite(
                SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(options)
                    .await
                    .map_err(|err| ApiError::internal("Failed to connect room database", err))?,
            )
        };
        let db = Self { backend };
        db.migrate().await?;
        Ok(db)
    }

    async fn execute(&self, sql: &str) -> ApiResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
        }
        .map_err(|err| ApiError::internal("Room DB execution", err))
    }

    async fn migrate(&self) -> ApiResult<()> {
        self.execute(
            r#"
            CREATE TABLE IF NOT EXISTS rooms (
                room_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                icon_json TEXT,
                members_json TEXT NOT NULL DEFAULT '[]',
                updated_at BIGINT NOT NULL DEFAULT 0,
                kind TEXT NOT NULL DEFAULT 'classic',
                description TEXT NOT NULL DEFAULT '',
                owner_id TEXT,
                roles_json TEXT NOT NULL DEFAULT '{}',
                bans_json TEXT NOT NULL DEFAULT '[]',
                timeouts_json TEXT NOT NULL DEFAULT '{}',
                chat_locked BIGINT NOT NULL DEFAULT 0,
                mod_permissions_json TEXT NOT NULL DEFAULT '{"canBan":true,"canKick":true,"canMute":true,"canDelete":true}',
                calls_enabled BIGINT NOT NULL DEFAULT 1
            )
            "#,
        )
        .await?;
        self.ensure_column("rooms", "kind", "kind TEXT NOT NULL DEFAULT 'classic'").await?;
        self.ensure_column("rooms", "description", "description TEXT NOT NULL DEFAULT ''").await?;
        self.ensure_column("rooms", "owner_id", "owner_id TEXT").await?;
        self.ensure_column("rooms", "roles_json", "roles_json TEXT NOT NULL DEFAULT '{}'").await?;
        self.ensure_column("rooms", "bans_json", "bans_json TEXT NOT NULL DEFAULT '[]'").await?;
        self.ensure_column("rooms", "timeouts_json", "timeouts_json TEXT NOT NULL DEFAULT '{}'").await?;
        self.ensure_column("rooms", "chat_locked", "chat_locked BIGINT NOT NULL DEFAULT 0").await?;
        self.ensure_column("rooms", "mod_permissions_json", "mod_permissions_json TEXT NOT NULL DEFAULT '{\"canBan\":true,\"canKick\":true,\"canMute\":true,\"canDelete\":true}'").await?;
        self.ensure_column("rooms", "calls_enabled", "calls_enabled BIGINT NOT NULL DEFAULT 1").await?;
        Ok(())
    }

    async fn ensure_column(&self, table: &str, column: &str, definition: &str) -> ApiResult<()> {
        let exists = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("Room DB schema query", err))?;
                rows.iter().any(|row| {
                    row.try_get::<String, _>("name")
                        .map(|name| name == column)
                        .unwrap_or(false)
                })
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = $2",
                )
                .bind(table)
                .bind(column)
                .fetch_optional(pool)
                .await
                .map_err(|err| ApiError::internal("Room DB schema query", err))?;
                row.is_some()
            }
        };
        if exists {
            return Ok(());
        }
        self.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
            .await
    }

    pub async fn room_record(&self, room_id: &str) -> Option<RoomRecord> {
        type Fields = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<i64>,
        );

        let fields: Fields = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query(
                    "SELECT room_id, title, icon_json, members_json, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled FROM rooms WHERE room_id = ?",
                )
                .bind(room_id)
                .fetch_optional(pool)
                .await
                .ok()??;
                (
                    row.try_get::<String, _>("room_id").ok()?,
                    row.try_get::<String, _>("title").ok(),
                    row.try_get::<Option<String>, _>("icon_json").ok().flatten(),
                    row.try_get::<String, _>("members_json").ok(),
                    row.try_get::<String, _>("kind").ok(),
                    row.try_get::<String, _>("description").ok(),
                    row.try_get::<Option<String>, _>("owner_id").ok().flatten(),
                    row.try_get::<String, _>("roles_json").ok(),
                    row.try_get::<String, _>("bans_json").ok(),
                    row.try_get::<String, _>("timeouts_json").ok(),
                    row.try_get::<i64, _>("chat_locked").ok(),
                    row.try_get::<String, _>("mod_permissions_json").ok(),
                    row.try_get::<i64, _>("calls_enabled").ok(),
                )
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT room_id, title, icon_json, members_json, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled FROM rooms WHERE room_id = $1",
                )
                .bind(room_id)
                .fetch_optional(pool)
                .await
                .ok()??;
                (
                    row.try_get::<String, _>("room_id").ok()?,
                    row.try_get::<String, _>("title").ok(),
                    row.try_get::<Option<String>, _>("icon_json").ok().flatten(),
                    row.try_get::<String, _>("members_json").ok(),
                    row.try_get::<String, _>("kind").ok(),
                    row.try_get::<String, _>("description").ok(),
                    row.try_get::<Option<String>, _>("owner_id").ok().flatten(),
                    row.try_get::<String, _>("roles_json").ok(),
                    row.try_get::<String, _>("bans_json").ok(),
                    row.try_get::<String, _>("timeouts_json").ok(),
                    row.try_get::<i64, _>("chat_locked").ok(),
                    row.try_get::<String, _>("mod_permissions_json").ok(),
                    row.try_get::<i64, _>("calls_enabled").ok(),
                )
            }
        };

        let (stored_room_id, title, icon_json, members_json, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled) = fields;
        let title = title.unwrap_or_else(|| stored_room_id.clone());
        let icon = icon_json
            .and_then(|value| serde_json::from_str::<RoomIcon>(&value).ok());
        let members = members_json
            .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .unwrap_or_default();
        let kind = kind
            .as_deref()
            .and_then(|value| serde_json::from_str::<RoomKind>(&format!("\"{value}\"" )).ok())
            .unwrap_or(RoomKind::Classic);
        let description = description.unwrap_or_default();
        let roles = roles_json
            .and_then(|value| serde_json::from_str::<BTreeMap<String, RoomRole>>(&value).ok())
            .unwrap_or_default();
        let banned = bans_json
            .and_then(|value| serde_json::from_str::<BTreeMap<String, String>>(&value).ok())
            .unwrap_or_default();
        let timeouts = timeouts_json
            .and_then(|value| serde_json::from_str::<BTreeMap<String, u64>>(&value).ok())
            .unwrap_or_default();
        let chat_locked = chat_locked.unwrap_or(0) != 0;
        let mod_permissions = mod_permissions_json
            .and_then(|value| serde_json::from_str::<ModeratorPermissions>(&value).ok())
            .unwrap_or_default();
        let calls_enabled = calls_enabled.unwrap_or(1) != 0;

        Some(RoomRecord {
            room_id: stored_room_id,
            title,
            icon,
            members,
            kind,
            description,
            owner_id,
            chat_locked,
            roles,
            banned,
            timeouts,
            mod_permissions,
            calls_enabled,
        })
    }

    pub async fn set_room_record(&self, room_id: &str, room: &RoomRecord) -> ApiResult<()> {
        let icon_json: Option<String> = room
            .icon
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| ApiError::internal("Icon json serialize", err))?;
        let members_json = serde_json::to_string(&room.members)
            .map_err(|err| ApiError::internal("Members json serialize", err))?;
        let roles_json = serde_json::to_string(&room.roles)
            .map_err(|err| ApiError::internal("Roles json serialize", err))?;
        let bans_json = serde_json::to_string(&room.banned)
            .map_err(|err| ApiError::internal("Bans json serialize", err))?;
        let timeouts_json = serde_json::to_string(&room.timeouts)
            .map_err(|err| ApiError::internal("Timeouts json serialize", err))?;
        let mod_permissions_json = serde_json::to_string(&room.mod_permissions)
            .map_err(|err| ApiError::internal("Moderator permissions json serialize", err))?;
        let kind = match room.kind {
            RoomKind::Classic => "classic",
            RoomKind::Community => "community",
        };
        let updated_at = now_ms() as i64;
        let chat_locked = if room.chat_locked { 1i64 } else { 0i64 };
        let calls_enabled = if room.calls_enabled { 1i64 } else { 0i64 };

        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(room_id) DO UPDATE SET title = excluded.title, icon_json = excluded.icon_json, members_json = excluded.members_json, updated_at = excluded.updated_at, kind = excluded.kind, description = excluded.description, owner_id = excluded.owner_id, roles_json = excluded.roles_json, bans_json = excluded.bans_json, timeouts_json = excluded.timeouts_json, chat_locked = excluded.chat_locked, mod_permissions_json = excluded.mod_permissions_json, calls_enabled = excluded.calls_enabled",
                )
                .bind(room_id)
                .bind(&room.title)
                .bind(icon_json)
                .bind(members_json)
                .bind(updated_at)
                .bind(kind)
                .bind(&room.description)
                .bind(room.owner_id.as_deref())
                .bind(roles_json)
                .bind(bans_json)
                .bind(timeouts_json)
                .bind(chat_locked)
                .bind(mod_permissions_json)
                .bind(calls_enabled)
                .execute(pool)
                .await
                .map(|_| ())
                .map_err(|err| ApiError::internal("Set room record", err))
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
                     ON CONFLICT(room_id) DO UPDATE SET title = excluded.title, icon_json = excluded.icon_json, members_json = excluded.members_json, updated_at = excluded.updated_at, kind = excluded.kind, description = excluded.description, owner_id = excluded.owner_id, roles_json = excluded.roles_json, bans_json = excluded.bans_json, timeouts_json = excluded.timeouts_json, chat_locked = excluded.chat_locked, mod_permissions_json = excluded.mod_permissions_json, calls_enabled = excluded.calls_enabled",
                )
                .bind(room_id)
                .bind(&room.title)
                .bind(icon_json)
                .bind(members_json)
                .bind(updated_at)
                .bind(kind)
                .bind(&room.description)
                .bind(room.owner_id.as_deref())
                .bind(roles_json)
                .bind(bans_json)
                .bind(timeouts_json)
                .bind(chat_locked)
                .bind(mod_permissions_json)
                .bind(calls_enabled)
                .execute(pool)
                .await
                .map(|_| ())
                .map_err(|err| ApiError::internal("Set room record", err))
            }
        }
    }

    pub async fn room_icon(&self, room_id: &str) -> Option<RoomIcon> {
        self.room_record(room_id).await.and_then(|room| room.icon)
    }

    pub async fn room_icon_uses_file_id(&self, file_id: &str) -> ApiResult<bool> {
        let pattern = format!("%\"id\":\"{}\"%", file_id.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let found = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(
                "SELECT 1 FROM rooms WHERE icon_json LIKE ? ESCAPE '\\' LIMIT 1",
            )
            .bind(pattern)
            .fetch_optional(pool)
            .await
            .map_err(|err| ApiError::internal("Room icon search", err))?
            .is_some(),
            SqlBackend::Postgres(pool) => sqlx::query(
                "SELECT 1 FROM rooms WHERE icon_json LIKE $1 ESCAPE '\\' LIMIT 1",
            )
            .bind(pattern)
            .fetch_optional(pool)
            .await
            .map_err(|err| ApiError::internal("Room icon search", err))?
            .is_some(),
        };
        Ok(found)
    }

    pub async fn set_room_icon(&self, room_id: &str, icon: &RoomIcon) -> ApiResult<()> {
        let mut room = self.room_record(room_id).await.unwrap_or(RoomRecord {
            room_id: room_id.to_owned(),
            title: room_id.to_owned(),
            ..Default::default()
        });
        room.icon = Some(icon.clone());
        self.set_room_record(room_id, &room).await
    }
}

async fn prepare_sqlite_path(url: &str) -> ApiResult<()> {
    let Some(raw_path) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    else {
        return Ok(());
    };
    if raw_path == ":memory:" || raw_path.trim().is_empty() {
        return Ok(());
    }
    if let Some(parent) = Path::new(raw_path).parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).await.map_err(|err| ApiError::internal("Prepare sqlite directory", err))?;
    }
    Ok(())
}
