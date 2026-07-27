use std::{path::Path, str::FromStr, sync::Arc};

use argon2::{
    password_hash::{
        rand_core::OsRng as PasswordOsRng, PasswordHash, PasswordHasher, PasswordVerifier,
        SaltString,
    },
    Argon2,
};
use base64::Engine as _;
use bip39::Language;
use rand::{rngs::OsRng, thread_rng, Rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{
    postgres::{PgPool, PgPoolOptions},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use tokio::fs;

use crate::{
    config::DatabaseConfig,
    models::{UserPresenceStatus, UserProfile},
    utils::now_ms,
};

pub type AccountResult<T> = Result<T, String>;

const SESSION_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const USERNAME_MIN: usize = 2;
const USERNAME_MAX: usize = 32;
const RESERVED_USERNAMES: &[&str] = &["system"];
const PASSWORD_MIN: usize = 8;
const PASSWORD_MAX: usize = 128;
const RECOVERY_WORD_COUNT: usize = 16;
const EARLY_USER_LIMIT: i64 = 200;
const MAX_USER_BADGES: usize = 16;
const MAX_USER_BADGE_LEN: usize = 32;
const USER_SELECT_BY_CREATED_DESC: &str = "SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users ORDER BY created_at DESC";
const USER_SELECT_BY_USERNAME_SQLITE: &str = "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) WHERE username = ?";
const USER_SELECT_BY_USERNAME_POSTGRES: &str = "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users WHERE username = $1";
const USER_SELECT_BY_ID_SQLITE: &str = "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) WHERE id = ?";
const USER_SELECT_BY_ID_POSTGRES: &str = "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users WHERE id = $1";

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
    ) -> AccountResult<Self> {
        let kind = config.kind.trim().to_ascii_lowercase();
        let backend = if kind == "postgres" || kind == "postgresql" {
            SqlBackend::Postgres(
                PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&config.url)
                    .await
                    .map_err(|err| format!("PostgreSQL connection failed: {err}"))?,
            )
        } else {
            prepare_sqlite_path(&config.url).await?;
            let options = SqliteConnectOptions::from_str(&config.url)
                .map_err(|err| format!("SQLite URL invalid: {err}"))?
                .create_if_missing(true);
            SqlBackend::Sqlite(
                SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(options)
                    .await
                    .map_err(|err| format!("SQLite connection failed: {err}"))?,
            )
        };

        let db = Self { backend, admin_ids };
        db.migrate().await?;
        db.ensure_feature_defaults(register_enabled).await?;
        Ok(db)
    }

    async fn migrate(&self) -> AccountResult<()> {
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
        Ok(())
    }

    async fn execute(&self, sql: &str) -> AccountResult<()> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query(sql).execute(pool).await.map(|_| ()),
        }
        .map_err(|err| format!("Database query failed: {err}"))
    }

    async fn ensure_column(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> AccountResult<()> {
        let exists = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
                    .fetch_all(pool)
                    .await
                    .map_err(|err| format!("Database schema query failed: {err}"))?;
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
                .map_err(|err| format!("Database schema query failed: {err}"))?;
                row.is_some()
            }
        };
        if exists {
            return Ok(());
        }
        self.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))
            .await
    }

    async fn ensure_feature_defaults(&self, register_enabled: bool) -> AccountResult<()> {
        self.set_feature_default("register_enabled", register_enabled)
            .await?;
        self.set_feature_default("calls_enabled", true).await?;
        Ok(())
    }

    async fn set_feature_default(&self, key: &str, enabled: bool) -> AccountResult<()> {
        let exists = self.feature_value(key).await?.is_some();
        if exists {
            return Ok(());
        }
        self.set_feature(key, enabled).await
    }

    pub async fn feature_flags(&self) -> AccountResult<FeatureFlags> {
        Ok(FeatureFlags {
            register_enabled: self
                .feature_value("register_enabled")
                .await?
                .unwrap_or(true),
            calls_enabled: self.feature_value("calls_enabled").await?.unwrap_or(true),
        })
    }

    async fn feature_value(&self, key: &str) -> AccountResult<Option<bool>> {
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT enabled FROM feature_flags WHERE key = ?")
                    .bind(key)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| format!("Database query failed: {err}"))?;
                Ok(row.map(|row| row.get::<i64, _>("enabled") != 0))
            }
            SqlBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT enabled FROM feature_flags WHERE key = $1")
                    .bind(key)
                    .fetch_optional(pool)
                    .await
                    .map_err(|err| format!("Database query failed: {err}"))?;
                Ok(row.map(|row| row.get::<i64, _>("enabled") != 0))
            }
        }
    }

    pub async fn set_feature(&self, key: &str, enabled: bool) -> AccountResult<()> {
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn register(
        &self,
        username: &str,
        password: &str,
    ) -> AccountResult<(PublicUser, String, Vec<String>)> {
        let username = validate_username(username)?;
        validate_password(password)?;
        if self.user_by_username(&username).await?.is_some() {
            return Err("Username is already taken.".to_owned());
        }

        let id = generate_snowflake_id();
        let recovery_words = generate_recovery_words();
        let recovery_phrase = recovery_words.join(" ");
        let password_hash = hash_secret(password)?;
        let recovery_hash = hash_secret(&recovery_phrase)?;
        let now = now_ms();
        let profile_json = serde_json::to_string(&UserProfile::default())
            .map_err(|err| format!("Could not encode profile: {err}"))?;

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
            return Err("Username is already taken.".to_owned());
        }

        let user = self
            .user_by_id(&id)
            .await?
            .ok_or("Account was not created.")?;
        let token = self.create_session(&id).await?;
        Ok((self.public_user(user), token, recovery_words))
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> AccountResult<(PublicUser, String)> {
        let username = normalize_username(username);
        let user = self
            .user_by_username(&username)
            .await?
            .ok_or_else(|| "Invalid username or password.".to_owned())?;
        if user.banned {
            return Err("Account is banned.".to_owned());
        }
        if user.disabled {
            return Err("Account is disabled.".to_owned());
        }
        verify_secret(password, &user.password_hash)
            .map_err(|_| "Invalid username or password.".to_owned())?;
        let token = self.create_session(&user.id).await?;
        Ok((self.public_user(user), token))
    }

    pub async fn recover(
        &self,
        username: &str,
        recovery_words: &str,
        new_password: &str,
    ) -> AccountResult<(PublicUser, String)> {
        validate_password(new_password)?;
        let username = normalize_username(username);
        let user = self
            .user_by_username(&username)
            .await?
            .ok_or_else(|| "Invalid recovery credentials.".to_owned())?;
        if user.banned {
            return Err("Account is banned.".to_owned());
        }
        verify_secret(
            &normalize_recovery_phrase(recovery_words),
            &user.recovery_hash,
        )
        .map_err(|_| "Invalid recovery credentials.".to_owned())?;
        let password_hash = hash_secret(new_password)?;
        let now = now_ms();
        self.update_password_hash(&user.id, &password_hash, now)
            .await?;
        let token = self.create_session(&user.id).await?;
        let updated = self
            .user_by_id(&user.id)
            .await?
            .ok_or("Account not found.")?;
        Ok((self.public_user(updated), token))
    }

    pub async fn authenticate_token(
        &self,
        token: &str,
    ) -> AccountResult<Option<AuthenticatedUser>> {
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
                .map_err(|err| format!("Database query failed: {err}"))?;
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
                .map_err(|err| format!("Database query failed: {err}"))?;
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

    pub async fn touch_session(&self, token: &str) -> AccountResult<()> {
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn logout(&self, token: &str) -> AccountResult<()> {
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn delete_account(&self, token: &str, password: &str) -> AccountResult<()> {
        let Some(user) = self.authenticate_token(token).await? else {
            return Err("Not authenticated.".to_owned());
        };
        let stored = self
            .user_by_id(&user.id)
            .await?
            .ok_or("Account not found.")?;
        verify_secret(password, &stored.password_hash)
            .map_err(|_| "Invalid password.".to_owned())?;
        self.delete_user_account(&user.id).await
    }

    pub async fn delete_user_account(&self, user_id: &str) -> AccountResult<()> {
        if self.user_by_id(user_id).await?.is_none() {
            return Err("Account not found.".to_owned());
        }
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("Database query failed: {err}"))?;
                sqlx::query("DELETE FROM users WHERE id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("Database query failed: {err}"))
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("Database query failed: {err}"))?;
                sqlx::query("DELETE FROM users WHERE id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|err| format!("Database query failed: {err}"))
            }
        }
    }

    pub async fn me(&self, token: &str) -> AccountResult<Option<(PublicUser, String)>> {
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
    ) -> AccountResult<Vec<(String, UserProfile)>> {
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
    ) -> AccountResult<Option<PublicUser>> {
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

    pub async fn profile_uses_file_id(&self, file_id: &str) -> AccountResult<bool> {
        let pattern = format!("%\"id\":\"{}\"%", file_id.replace('%', "\\%").replace('_', "\\_"));
        let found = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("SELECT 1 FROM users WHERE profile_json LIKE ? LIMIT 1")
                .bind(&pattern)
                .fetch_optional(pool)
                .await
                .map(|row| row.is_some()),
            SqlBackend::Postgres(pool) => sqlx::query("SELECT 1 FROM users WHERE profile_json LIKE $1 LIMIT 1")
                .bind(&pattern)
                .fetch_optional(pool)
                .await
                .map(|row| row.is_some()),
        }
        .map_err(|err| format!("Database query failed: {err}"))?;
        Ok(found)
    }

    pub async fn update_profile(&self, user_id: &str, profile: &UserProfile) -> AccountResult<()> {
        let profile_json = serde_json::to_string(profile)
            .map_err(|err| format!("Could not encode profile: {err}"))?;
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn update_status(
        &self,
        user_id: &str,
        status: UserPresenceStatus,
    ) -> AccountResult<()> {
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn change_username(
        &self,
        user_id: &str,
        username: &str,
    ) -> AccountResult<PublicUser> {
        let username = validate_username(username)?;
        if let Some(existing) = self.user_by_username(&username).await? {
            if existing.id != user_id {
                return Err("Username is already taken.".to_owned());
            }
        }
        let mut user = self
            .user_by_id(user_id)
            .await?
            .ok_or("Account not found.")?;
        if user.username == username {
            return Ok(self.public_user(user));
        }
        let now = now_ms();
        let window_start = now.saturating_sub(7 * 24 * 60 * 60 * 1000);
        user.username_changes.retain(|stamp| *stamp >= window_start);
        if !user.username_changes.is_empty() {
            return Err("Username can only be changed once per week.".to_owned());
        }
        user.username_changes.push(now);
        let changes_json = serde_json::to_string(&user.username_changes)
            .map_err(|err| format!("Could not encode username changes: {err}"))?;
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
        .map_err(|err| format!("Database query failed: {err}"))?;
        let updated = self
            .user_by_id(user_id)
            .await?
            .ok_or("Account not found.")?;
        Ok(self.public_user(updated))
    }

    pub async fn list_users(&self) -> AccountResult<Vec<PublicUser>> {
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
        .map_err(|err| format!("Database query failed: {err}"))?;
        rows.into_iter()
            .map(|row| self.stored_from_raw(row).map(|user| self.public_user(user)))
            .collect()
    }

    pub async fn set_user_disabled(&self, user_id: &str, disabled: bool) -> AccountResult<()> {
        let value = if disabled { 1i64 } else { 0i64 };
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET disabled = ?, updated_at = ? WHERE id = ?")
                    .bind(value)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET disabled = $1, updated_at = $2 WHERE id = $3")
                    .bind(value)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| format!("Database query failed: {err}"))
    }

    pub async fn set_user_badges(&self, user_id: &str, badges: &[String]) -> AccountResult<PublicUser> {
        let badges = sanitize_custom_badges(badges)?;
        let badges_json = serde_json::to_string(&badges)
            .map_err(|err| format!("Could not encode badges: {err}"))?;
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET custom_badges_json = ?, updated_at = ? WHERE id = ?")
                    .bind(badges_json)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET custom_badges_json = $1, updated_at = $2 WHERE id = $3")
                    .bind(badges_json)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| format!("Database query failed: {err}"))?;
        let updated = self
            .user_by_id(user_id)
            .await?
            .ok_or("Account not found.")?;
        Ok(self.public_user(updated))
    }

    pub async fn set_user_banned(&self, user_id: &str, banned: bool) -> AccountResult<()> {
        let value = if banned { 1i64 } else { 0i64 };
        let now = now_ms() as i64;
        match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET banned = ?, updated_at = ? WHERE id = ?")
                    .bind(value)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query("UPDATE users SET banned = $1, updated_at = $2 WHERE id = $3")
                    .bind(value)
                    .bind(now)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(|err| format!("Database query failed: {err}"))?;

        if banned {
            match &self.backend {
                SqlBackend::Sqlite(pool) => sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ()),
                SqlBackend::Postgres(pool) => sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map(|_| ()),
            }
            .map_err(|err| format!("Database query failed: {err}"))?;
        }

        Ok(())
    }

    fn is_admin(&self, user_id: &str) -> bool {
        self.admin_ids.iter().any(|id| id == user_id)
    }

    fn user_badges(&self, user: &StoredUser) -> Vec<String> {
        let mut badges = Vec::new();
        if self.is_admin(&user.id) {
            badges.push("admin".to_owned());
        }
        if user.user_rank > 0 && user.user_rank <= EARLY_USER_LIMIT {
            badges.push("early".to_owned());
        }
        for badge in &user.custom_badges {
            if !badges.iter().any(|existing| existing == badge) {
                badges.push(badge.clone());
            }
        }
        badges
    }

    fn public_user(&self, user: StoredUser) -> PublicUser {
        let badges = self.user_badges(&user);
        PublicUser {
            id: user.id.clone(),
            username: user.username,
            profile: user.profile,
            status: user.status,
            disabled: user.disabled,
            banned: user.banned,
            admin: self.is_admin(&user.id),
            badges,
            created_at: user.created_at,
        }
    }

    async fn create_session(&self, user_id: &str) -> AccountResult<String> {
        let token = random_token();
        let hash = token_hash(&token);
        let now = now_ms();
        let expires = now + SESSION_TTL_MS;
        match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
                .bind(hash)
                .bind(user_id)
                .bind(now as i64)
                .bind(expires as i64)
                .execute(pool)
                .await
                .map(|_| ()),
            SqlBackend::Postgres(pool) => sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES ($1, $2, $3, $4)")
                .bind(hash)
                .bind(user_id)
                .bind(now as i64)
                .bind(expires as i64)
                .execute(pool)
                .await
                .map(|_| ()),
        }
        .map_err(|err| format!("Database query failed: {err}"))?;
        Ok(token)
    }

    async fn update_password_hash(
        &self,
        user_id: &str,
        password_hash: &str,
        now: u64,
    ) -> AccountResult<()> {
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
        .map_err(|err| format!("Database query failed: {err}"))
    }

    async fn user_by_username(&self, username: &str) -> AccountResult<Option<StoredUser>> {
        let row = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_USERNAME_SQLITE)
                    .bind(username)
                    .fetch_optional(pool)
                    .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_USERNAME_POSTGRES)
                    .bind(username)
                    .fetch_optional(pool)
                    .await
            }
        }
        .map_err(|err| format!("Database query failed: {err}"))?;
        row.map(|row| self.stored_from_raw(row)).transpose()
    }

    async fn user_by_id(&self, user_id: &str) -> AccountResult<Option<StoredUser>> {
        let row = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_ID_SQLITE)
                    .bind(user_id)
                    .fetch_optional(pool)
                    .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, RawStoredUser>(USER_SELECT_BY_ID_POSTGRES)
                    .bind(user_id)
                    .fetch_optional(pool)
                    .await
            }
        }
        .map_err(|err| format!("Database query failed: {err}"))?;
        row.map(|row| self.stored_from_raw(row)).transpose()
    }

    fn stored_from_raw(&self, row: RawStoredUser) -> AccountResult<StoredUser> {
        let profile = serde_json::from_str::<UserProfile>(&row.profile_json).unwrap_or_default();
        let status = parse_status(&row.status);
        let username_changes =
            serde_json::from_str::<Vec<u64>>(&row.username_changes_json).unwrap_or_default();
        let custom_badges = serde_json::from_str::<Vec<String>>(&row.custom_badges_json)
            .map(|badges| sanitize_custom_badges(&badges).unwrap_or_default())
            .unwrap_or_default();
        Ok(StoredUser {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            recovery_hash: row.recovery_hash,
            profile,
            status,
            disabled: row.disabled != 0,
            banned: row.banned != 0,
            created_at: row.created_at.max(0) as u64,
            user_rank: row.user_rank,
            username_changes,
            custom_badges,
        })
    }
}

async fn prepare_sqlite_path(url: &str) -> AccountResult<()> {
    let Some(path) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    else {
        return Ok(());
    };
    if path == ":memory:" {
        return Ok(());
    }
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| format!("Could not create SQLite directory: {err}"))?;
    }
    Ok(())
}

fn sanitize_custom_badges(raw: &[String]) -> AccountResult<Vec<String>> {
    let mut badges = Vec::new();
    for value in raw {
        let badge = value.trim().to_ascii_lowercase();
        if badge.is_empty()
            || matches!(badge.as_str(), "staff" | "admin" | "system")
            || badges.iter().any(|existing| existing == &badge)
        {
            continue;
        }
        if badge.chars().count() > MAX_USER_BADGE_LEN {
            return Err("Badge name is too long.".to_owned());
        }
        if !badge
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return Err("Badge name may only contain letters, numbers, '-' or '_'.".to_owned());
        }
        badges.push(badge);
        if badges.len() > MAX_USER_BADGES {
            return Err("Too many badges.".to_owned());
        }
    }
    Ok(badges)
}

pub fn validate_username(raw: &str) -> AccountResult<String> {
    let username = normalize_username(raw);
    let len = username.chars().count();
    if len < USERNAME_MIN || len > USERNAME_MAX {
        return Err(format!(
            "Username must be between {USERNAME_MIN} and {USERNAME_MAX} characters."
        ));
    }
    if !username
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err("Username may only contain letters, numbers, '-', '_' or '.'.".to_owned());
    }
    if RESERVED_USERNAMES.contains(&username.as_str()) {
        return Err("Username is reserved.".to_owned());
    }
    Ok(username)
}

pub fn normalize_username(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

pub fn username_hits_blocklist(username: &str, terms: &[String]) -> bool {
    let candidate = normalize_username(username);
    terms
        .iter()
        .map(|item| normalize_username(item))
        .any(|blocked| !blocked.is_empty() && candidate.contains(&blocked))
}

pub fn normalize_recovery_phrase(raw: &str) -> String {
    raw.split_whitespace()
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_password(password: &str) -> AccountResult<()> {
    let len = password.chars().count();
    if len < PASSWORD_MIN || len > PASSWORD_MAX {
        return Err(format!(
            "Password must be between {PASSWORD_MIN} and {PASSWORD_MAX} characters."
        ));
    }
    Ok(())
}

fn generate_recovery_words() -> Vec<String> {
    let words = Language::English.word_list();
    let mut rng = thread_rng();
    (0..RECOVERY_WORD_COUNT)
        .map(|_| {
            let idx = rng.gen_range(0..words.len());
            words[idx].to_owned()
        })
        .collect()
}

fn hash_secret(secret: &str) -> AccountResult<String> {
    let salt = SaltString::generate(&mut PasswordOsRng);
    Argon2::default()
        .hash_password(secret.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| format!("Hashing failed: {err}"))
}

fn verify_secret(secret: &str, hash: &str) -> AccountResult<()> {
    let parsed = PasswordHash::new(hash).map_err(|err| format!("Hash parse failed: {err}"))?;
    Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .map_err(|err| format!("Verification failed: {err}"))
}

fn generate_snowflake_id() -> String {
    let now = now_ms();
    let mut rng = OsRng;
    let random: u16 = rng.next_u32() as u16;
    let value = (now << 12) | u64::from(random & 0x0fff);
    value.to_string()
}

fn random_token() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn parse_status(raw: &str) -> UserPresenceStatus {
    match raw.trim().to_ascii_lowercase().as_str() {
        "invisible" => UserPresenceStatus::Invisible,
        "dnd" => UserPresenceStatus::Dnd,
        _ => UserPresenceStatus::Online,
    }
}

fn status_to_str(status: UserPresenceStatus) -> &'static str {
    match status {
        UserPresenceStatus::Online => "online",
        UserPresenceStatus::Invisible => "invisible",
        UserPresenceStatus::Dnd => "dnd",
    }
}

pub fn user_response(user: PublicUser, token: String) -> serde_json::Value {
    json!({
        "ok": true,
        "token": token,
        "user": user
    })
}

pub type SharedAccounts = Arc<AccountDatabase>;
