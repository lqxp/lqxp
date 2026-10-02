use std::{
    collections::BTreeMap,
    path::{PathBuf},
    str::FromStr,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgPool, PgPoolOptions},
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    ColumnIndex, FromRow, Row,
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
    /// The badges actually stored on the account, as opposed to the ones the
    /// server derives (`admin` from the admin list, `early` from the account
    /// rank). The admin panel needs the difference to know which badges it can
    /// take away. Only the account itself and admins ever receive this.
    pub custom_badges: Vec<String>,
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
    pub custom_badges: Vec<String>,
    pub created_at: u64,
}

/// One day of account creations, read back from `users.created_at`.
///
/// An aggregate over rows the table already holds: it counts, it does not
/// record. Nothing new is written, no account is identifiable in the result,
/// and the buckets are UTC days so the answer does not depend on where the
/// server happens to run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignupDay {
    /// Midnight UTC of the day, in epoch milliseconds.
    pub day: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStats {
    pub total: u64,
    pub disabled: u64,
    pub banned: u64,
    pub new_last_day: u64,
    pub new_last_week: u64,
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

/// Default room ("default session"): roomId + roomKey, hot-configurable by an
/// admin. The server deliberately holds the official room's E2EE `room_key`
/// to redistribute it on each connection (accepted by design for a
/// non-sensitive news channel).
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

/// Sort order for the admin user browse. The keyset cursor always matches
/// the active sort so pages stay stable while accounts are created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserListSort {
    /// Newest first (`created_at DESC, id DESC`).
    #[default]
    Newest,
    /// Oldest first (`created_at ASC, id ASC`).
    Oldest,
    /// Case-insensitive username (`LOWER(username) ASC, id ASC`).
    Username,
}

/// Account-state filter for the admin user browse. `Admin` matches the
/// configured admin id list; every other variant reads the stored flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserListStatus {
    #[default]
    All,
    Active,
    Disabled,
    Banned,
    Admin,
}

/// Server-side filters for the admin user browse
/// (`GET /api/admin/users`). Everything here is a pure SQL predicate, so
/// filtered pages stay full-or-exhausted and the client's `page < limit`
/// exhaustion heuristic keeps working at any table size.
#[derive(Debug, Clone, Default)]
pub struct UserListFilter {
    /// Case-insensitive substring over `username` and `id`.
    pub query: String,
    pub status: UserListStatus,
    /// Substring over the stored custom badges, plus the computed `admin`
    /// and `early` badge names (matched against the admin id list and the
    /// global creation rank, exactly like `user_badges` derives them).
    pub badge: String,
    /// Inclusive `created_at` bounds, epoch milliseconds.
    pub created_after: Option<i64>,
    pub created_before: Option<i64>,
    pub sort: UserListSort,
}

impl UserListFilter {
    /// Builds a filter from raw query params, rejecting garbage with a
    /// static 400 message (never interpolate user input into errors).
    pub fn from_params(
        q: Option<&str>,
        status: Option<&str>,
        badge: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        sort: Option<&str>,
    ) -> ApiResult<Self> {
        let query = q.unwrap_or("").trim().chars().take(64).collect::<String>();
        let status = match status.unwrap_or("all").trim().to_lowercase().as_str() {
            "all" => UserListStatus::All,
            "active" => UserListStatus::Active,
            "disabled" => UserListStatus::Disabled,
            "banned" => UserListStatus::Banned,
            "admin" => UserListStatus::Admin,
            _ => return Err(ApiError::bad_request("Invalid status.")),
        };
        let badge = badge
            .unwrap_or("")
            .trim()
            .to_lowercase()
            .chars()
            .take(64)
            .collect::<String>();
        let parse_bound = |raw: Option<&str>, name: &str| -> ApiResult<Option<i64>> {
            match raw {
                None => Ok(None),
                Some(text) => {
                    let value = text
                        .trim()
                        .parse::<i64>()
                        .map_err(|_| ApiError::bad_request(name))?;
                    if value < 0 {
                        return Err(ApiError::bad_request(name));
                    }
                    Ok(Some(value))
                }
            }
        };
        let created_after = parse_bound(from, "Invalid from.")?;
        let created_before = parse_bound(to, "Invalid to.")?;
        if let (Some(after), Some(before)) = (created_after, created_before) {
            if after > before {
                return Err(ApiError::bad_request("Invalid range."));
            }
        }
        let sort = match sort.unwrap_or("newest").trim().to_lowercase().as_str() {
            "newest" => UserListSort::Newest,
            "oldest" => UserListSort::Oldest,
            "username" => UserListSort::Username,
            _ => return Err(ApiError::bad_request("Invalid sort.")),
        };
        Ok(Self {
            query,
            status,
            badge,
            created_after,
            created_before,
            sort,
        })
    }
}

/// Keyset position for the admin user browse. The encoding is tagged so a
/// cursor is only ever read back with the sort order that wrote it; the
/// pre-tag `{created}:{id}` shape is still accepted as a time cursor so a
/// client mid-pagination across a server upgrade does not get a 400.
enum UserListCursor {
    Time { created: i64, id: String },
    Name { username: String, id: String },
}

/// String that sorts after every plausible account id on first DESC pages.
const CURSOR_HIGH_ID: &str = "\u{10FFFF}";

fn decode_user_cursor(raw: &str) -> ApiResult<Option<UserListCursor>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let invalid = || ApiError::bad_request("Invalid cursor.");
    let check_id = |id: &str| -> ApiResult<String> {
        if id.is_empty() || id.len() > 64 {
            return Err(invalid());
        }
        Ok(id.to_owned())
    };
    if let Some(rest) = raw.strip_prefix("t:") {
        let (created_raw, id_raw) = rest.split_once(':').ok_or_else(invalid)?;
        let created = created_raw.parse::<i64>().map_err(|_| invalid())?;
        return Ok(Some(UserListCursor::Time {
            created,
            id: check_id(id_raw)?,
        }));
    }
    if let Some(rest) = raw.strip_prefix("u:") {
        // Split at the last colon: legacy usernames can hold anything,
        // ids (snowflakes) never contain one.
        let (name_raw, id_raw) = rest.rsplit_once(':').ok_or_else(invalid)?;
        if name_raw.is_empty() || name_raw.chars().count() > 128 {
            return Err(invalid());
        }
        return Ok(Some(UserListCursor::Name {
            username: name_raw.to_owned(),
            id: check_id(id_raw)?,
        }));
    }
    let (created_raw, id_raw) = raw.split_once(':').ok_or_else(invalid)?;
    let created = created_raw.parse::<i64>().map_err(|_| invalid())?;
    Ok(Some(UserListCursor::Time {
        created,
        id: check_id(id_raw)?,
    }))
}

/// One bound value for the dynamically built listing query. Variants exist
/// so conditions can be assembled in a fixed order for both backends.
enum ListBind {
    Int(i64),
    Text(String),
}

/// Applies a dynamic bind list to a concrete query. Must stay a macro:
/// `Query::bind` needs `Encode + Type` for the concrete database, which no
/// generic helper can promise for both backends at once.
macro_rules! bind_all {
    ($query:ident, $binds:expr) => {{
        for bind in $binds {
            $query = match bind {
                ListBind::Int(value) => $query.bind(value),
                ListBind::Text(value) => $query.bind(value),
            };
        }
    }};
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
            ensure_sqlite_database(&config.url, config.create_if_missing).await?;
            let options = SqliteConnectOptions::from_str(&config.url)
                .map_err(|err| ApiError::internal("SQLite URL invalid", err))?
                .create_if_missing(config.create_if_missing)
                // Under production write load a reader can hit SQLITE_BUSY
                // while a writer holds the database lock. Retry in-process
                // for a few seconds instead of failing the request: without
                // this every endpoint 500s the moment two connections collide.
                .busy_timeout(Duration::from_secs(5));
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

        // QXP-PHANTOM: public prekeys, opaque block tags, roster blob.
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

        // Case is not identity: two accounts must never differ by case only,
        // otherwise the second makes the first unreachable (all reads go
        // through `normalize_username`).
        // Writes already normalize (`security::validate_username_with_max`);
        // this index covers out-of-application paths (manual SQL, backup
        // restore, tooling).
        //
        // Non-blocking and rename-free: if legacy case variants block the
        // index, report it instead of refusing to start.
        let uniqueness_guard = match &self.backend {
            SqlBackend::Sqlite(_) => {
                "CREATE UNIQUE INDEX IF NOT EXISTS users_username_nocase ON users (username COLLATE NOCASE)"
            }
            SqlBackend::Postgres(_) => {
                "CREATE UNIQUE INDEX IF NOT EXISTS users_username_lower ON users (LOWER(username))"
            }
        };
        if let Err(err) = self.execute(uniqueness_guard).await {
            tracing::warn!("Case-insensitive username uniqueness is not enforced: {err}");
        }
        self.warn_about_legacy_username_casing().await;

        Ok(())
    }

    /// Reports accounts whose username is not lowercase.
    ///
    /// These rows are unreachable: `login` and `recover` look up the lowercase
    /// form. No automatic rename happens here — renaming an account is an
    /// operator decision, not a startup side effect — but the anomaly must not
    /// stay silent.
    async fn warn_about_legacy_username_casing(&self) {
        let query = "SELECT COUNT(*) FROM users WHERE username <> LOWER(username)";
        let count = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query(query)
                .fetch_one(pool)
                .await
                .map(|row| row.get::<i64, _>(0)),
            SqlBackend::Postgres(pool) => sqlx::query(query)
                .fetch_one(pool)
                .await
                .map(|row| row.get::<i64, _>(0)),
        };
        match count {
            Ok(0) => {}
            Ok(count) => tracing::warn!(
                "{count} account(s) have a username that is not lowercase and cannot be \
                 reached by login or recovery; rename them to their lowercase form."
            ),
            Err(err) => tracing::warn!("Could not check username casing: {err}"),
        }
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
            custom_badges: user.custom_badges.clone(),
            created_at: user.created_at,
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
                    "SELECT bundle_json FROM prekeys WHERE user_id = ?",
                )
                .bind(user_id)
                .fetch_optional(pool)
                .await
            }
            SqlBackend::Postgres(pool) => {
                sqlx::query_as::<_, StoredPrekey>(
                    "SELECT bundle_json FROM prekeys WHERE user_id = $1",
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

    /// Returns `Ok(Some(current_ver))` on LWW conflict, `Ok(None)` otherwise.
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

    /// Bulk account removal, filtered in SQL.
    ///
    /// The date window and the username pattern are pushed down to the
    /// database so only candidate rows are loaded; the length check and the
    /// admin exclusion then run over that much smaller set.
    pub async fn purge_accounts(
        &self,
        created_after_ms: Option<u64>,
        created_before_ms: Option<u64>,
        min_username_len: Option<usize>,
        max_username_len: Option<usize>,
        username_contains: Option<&str>,
        exclude_admin: bool,
    ) -> ApiResult<usize> {
        let after = created_after_ms.map(|value| value as i64).unwrap_or(i64::MIN);
        let before = created_before_ms.map(|value| value as i64).unwrap_or(i64::MAX);
        let like = username_contains
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", escape_like_pattern(&value.to_lowercase())))
            .unwrap_or_else(|| "%".to_owned());

        let candidates: Vec<(String, String)> = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query_as(
                "SELECT id, username FROM users WHERE created_at >= ? AND created_at <= ? AND LOWER(username) LIKE ? ESCAPE '\\'",
            )
            .bind(after)
            .bind(before)
            .bind(&like)
            .fetch_all(pool)
            .await,
            SqlBackend::Postgres(pool) => sqlx::query_as(
                "SELECT id, username FROM users WHERE created_at >= $1 AND created_at <= $2 AND LOWER(username) LIKE $3 ESCAPE '\\'",
            )
            .bind(after)
            .bind(before)
            .bind(&like)
            .fetch_all(pool)
            .await,
        }
        .map_err(|err| ApiError::internal("Purge candidate query", err))?;

        let mut count = 0usize;
        for (id, username) in candidates {
            if exclude_admin && self.is_admin(&id) {
                continue;
            }
            let char_len = username.chars().count();
            if min_username_len.is_some_and(|min| char_len < min) {
                continue;
            }
            if max_username_len.is_some_and(|max| char_len > max) {
                continue;
            }
            if self.delete_user_account(&id).await.is_ok() {
                count += 1;
            }
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
                custom_badges: user.custom_badges,
                created_at: user.created_at,
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

    /// Accounts created per day over the last `days` days, oldest first.
    ///
    /// Every day in the window comes back, quiet ones as zero, so a chart has
    /// a continuous series instead of gaps it would have to invent a line
    /// across. Bucketing is integer arithmetic rather than a date function,
    /// because SQLite and Postgres do not spell those the same way.
    pub async fn signups_per_day(&self, days: u32) -> ApiResult<Vec<SignupDay>> {
        const DAY_MS: i64 = 24 * 60 * 60 * 1000;
        let span = days.clamp(1, 120) as i64;
        let today = (now_ms() as i64 / DAY_MS) * DAY_MS;
        let origin = today - (span - 1) * DAY_MS;
        let rows: Vec<(i64, i64)> = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query_as(
                "SELECT (created_at - ?) / 86400000 AS bucket, COUNT(*) FROM users \
                 WHERE created_at >= ? GROUP BY 1 ORDER BY 1",
            )
            .bind(origin)
            .bind(origin)
            .fetch_all(pool)
            .await,
            SqlBackend::Postgres(pool) => sqlx::query_as(
                "SELECT (created_at - $1) / 86400000 AS bucket, CAST(COUNT(*) AS BIGINT) FROM users \
                 WHERE created_at >= $2 GROUP BY 1 ORDER BY 1",
            )
            .bind(origin)
            .bind(origin)
            .fetch_all(pool)
            .await,
        }
        .map_err(|err| ApiError::internal("Signup histogram query", err))?;

        let mut counts = vec![0u64; span as usize];
        for (bucket, count) in rows {
            if bucket >= 0 && (bucket as usize) < counts.len() {
                counts[bucket as usize] = count.max(0) as u64;
            }
        }
        Ok(counts
            .into_iter()
            .enumerate()
            .map(|(index, count)| SignupDay {
                day: (origin + index as i64 * DAY_MS) as u64,
                count,
            })
            .collect())
    }

    pub async fn user_stats(&self) -> ApiResult<UserStats> {
        let now = now_ms() as i64;
        let day_ago = now - 24 * 60 * 60 * 1000;
        let week_ago = now - 7 * 24 * 60 * 60 * 1000;
        let row: (i64, i64, i64, i64, i64) = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query_as(
                "SELECT COUNT(*), \
                 COALESCE(SUM(CASE WHEN disabled != 0 THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN banned != 0 THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN created_at >= ? THEN 1 ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN created_at >= ? THEN 1 ELSE 0 END), 0) FROM users",
            )
            .bind(day_ago)
            .bind(week_ago)
            .fetch_one(pool)
            .await,
            SqlBackend::Postgres(pool) => sqlx::query_as(
                "SELECT COUNT(*), \
                 CAST(COALESCE(SUM(CASE WHEN disabled != 0 THEN 1 ELSE 0 END), 0) AS BIGINT), \
                 CAST(COALESCE(SUM(CASE WHEN banned != 0 THEN 1 ELSE 0 END), 0) AS BIGINT), \
                 CAST(COALESCE(SUM(CASE WHEN created_at >= $1 THEN 1 ELSE 0 END), 0) AS BIGINT), \
                 CAST(COALESCE(SUM(CASE WHEN created_at >= $2 THEN 1 ELSE 0 END), 0) AS BIGINT) FROM users",
            )
            .bind(day_ago)
            .bind(week_ago)
            .fetch_one(pool)
            .await,
        }
        .map_err(|err| ApiError::internal("User stats query", err))?;
        Ok(UserStats {
            total: row.0.max(0) as u64,
            disabled: row.1.max(0) as u64,
            banned: row.2.max(0) as u64,
            new_last_day: row.3.max(0) as u64,
            new_last_week: row.4.max(0) as u64,
        })
    }

    /// Admin username search: never loads the whole table. Bounded SQL
    /// candidates (exact → prefix → substring) are then ranked in Rust by
    /// edit distance, so typos still surface and only the top `limit`
    /// matches are returned.
    pub async fn search_users(&self, query: &str, limit: usize) -> ApiResult<(Vec<PublicUser>, u64)> {
        // Usernames are capped at 32 chars server-side; bound the needle so
        // huge queries can't turn into expensive LIKE scans.
        let needle: String = query.trim().to_lowercase().chars().take(64).collect();
        if needle.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let limit = limit.clamp(1, 50);
        // Ask the DB for a few extra candidates so the in-Rust ranking has
        // room to promote close matches.
        let sql_limit = (limit * 4).max(100) as i64;
        let prefix = format!("{}%", escape_like_pattern(&needle));
        let substring = format!("%{}%", escape_like_pattern(&needle));

        let select = "SELECT * FROM (SELECT id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users";
        // Rows decode independently per backend (SqliteRow vs PgRow never
        // unify), then share the resilient decoder below.
        let (stored, skipped) = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let sql = format!(
                    "{select} WHERE LOWER(username) = ? OR LOWER(username) LIKE ? ESCAPE '\\' OR LOWER(username) LIKE ? ESCAPE '\\' ORDER BY CASE WHEN LOWER(username) = ? THEN 0 WHEN LOWER(username) LIKE ? ESCAPE '\\' THEN 1 ELSE 2 END ASC, created_at DESC LIMIT ?"
                );
                let rows = sqlx::query(&sql)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(&substring)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(sql_limit)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("Search users query", err))?;
                self.decode_stored_rows(rows)
            }
            SqlBackend::Postgres(pool) => {
                let sql = format!(
                    "{select} WHERE LOWER(username) = $1 OR LOWER(username) LIKE $2 ESCAPE '\\' OR LOWER(username) LIKE $3 ESCAPE '\\' ORDER BY CASE WHEN LOWER(username) = $1 THEN 0 WHEN LOWER(username) LIKE $2 ESCAPE '\\' THEN 1 ELSE 2 END ASC, created_at DESC LIMIT $4"
                );
                let rows = sqlx::query(&sql)
                    .bind(&needle)
                    .bind(&prefix)
                    .bind(&substring)
                    .bind(sql_limit)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("Search users query", err))?;
                self.decode_stored_rows(rows)
            }
        };

        // One malformed row must not fail the whole search: decode per row,
        // skip what does not parse, and report how many were skipped.
        let mut ranked: Vec<(u8, usize, StoredUser)> = Vec::with_capacity(stored.len());
        for user in stored {
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
        let users = ranked
            .into_iter()
            .map(|(_, _, user)| self.public_user(user))
            .collect();
        Ok((users, skipped))
    }


    /// Decodes one batch of user rows, skipping malformed rows instead of
    /// failing the whole page. A single legacy/hand-edited row (NULL or a
    /// mistyped value where the schema says otherwise) used to turn every
    /// admin listing into a 500; now it is warned about and counted.
    fn decode_stored_rows<R>(&self, rows: Vec<R>) -> (Vec<StoredUser>, u64)
    where
        R: sqlx::Row,
        RawStoredUser: for<'r> FromRow<'r, R>,
        for<'r> &'r str: ColumnIndex<R>,
        String: sqlx::Type<<R as sqlx::Row>::Database>,
        for<'r> String: sqlx::Decode<'r, <R as sqlx::Row>::Database>,
    {
        let mut users = Vec::with_capacity(rows.len());
        let mut skipped = 0u64;
        for row in &rows {
            match RawStoredUser::from_row(row) {
                Ok(raw) => match self.stored_from_raw(raw) {
                    Ok(user) => users.push(user),
                    Err(err) => {
                        skipped += 1;
                        tracing::warn!("Skipping malformed user row: {err}");
                    }
                },
                Err(err) => {
                    skipped += 1;
                    let id = row
                        .try_get::<String, _>("id")
                        .unwrap_or_else(|_| "?".to_owned());
                    tracing::warn!("Skipping malformed user row id={id}: {err}");
                }
            }
        }
        (users, skipped)
    }

    /// Paginated full-table browse for the admin user center
    /// (`GET /api/admin/users`). Keyset on `(created_at, id)` for time sorts
    /// and `(LOWER(username), id)` for username sort, so pages stay stable
    /// while accounts are created. Returns the page plus the cursor for the
    /// next page (if any) plus the count of malformed rows skipped.
    pub async fn list_users_page(
        &self,
        filter: &UserListFilter,
        limit: usize,
        cursor: Option<&str>,
    ) -> ApiResult<(Vec<PublicUser>, Option<String>, u64)> {
        let limit = limit.clamp(1, 100);
        let position = decode_user_cursor(cursor.unwrap_or(""))?;
        // created_at is stored as BIGINT ms; ids are numeric snowflakes, so
        // lexicographic id comparison matches numeric order here.
        let (after_created, after_id, after_name) = match (filter.sort, position) {
            (UserListSort::Newest, None) => (i64::MAX, CURSOR_HIGH_ID.to_owned(), String::new()),
            (UserListSort::Newest, Some(UserListCursor::Time { created, id })) => (created, id, String::new()),
            (UserListSort::Oldest, None) => (i64::MIN, String::new(), String::new()),
            (UserListSort::Oldest, Some(UserListCursor::Time { created, id })) => (created, id, String::new()),
            (UserListSort::Username, None) => (0, String::new(), String::new()),
            (UserListSort::Username, Some(UserListCursor::Name { username, id })) => (0, id, username),
            // A cursor written by another sort order cannot be honored.
            _ => return Err(ApiError::bad_request("Invalid cursor.")),
        };

        // Conditions are assembled once, rendered per backend (`?` vs `$n`).
        // `user_rank` comes from the same full-table window the search uses,
        // so the `early` badge check matches `user_badges` exactly.
        enum Cond {
            Keyset,
            Query,
            Active,
            Disabled,
            Banned,
            Admin,
            Badge,
            From,
            To,
        }
        let mut conds: Vec<Cond> = vec![Cond::Keyset];
        if !filter.query.is_empty() {
            conds.push(Cond::Query);
        }
        match filter.status {
            UserListStatus::All => {}
            UserListStatus::Active => conds.push(Cond::Active),
            UserListStatus::Disabled => conds.push(Cond::Disabled),
            UserListStatus::Banned => conds.push(Cond::Banned),
            UserListStatus::Admin => {
                if self.admin_ids.is_empty() {
                    return Ok((Vec::new(), None, 0));
                }
                conds.push(Cond::Admin);
            }
        }
        if !filter.badge.is_empty() {
            conds.push(Cond::Badge);
        }
        if filter.created_after.is_some() {
            conds.push(Cond::From);
        }
        if filter.created_before.is_some() {
            conds.push(Cond::To);
        }

        let needle = filter.query.to_lowercase();
        let needle_like = format!("%{}%", escape_like_pattern(&needle));
        let badge_like = format!("%{}%", escape_like_pattern(&filter.badge));
        let admin_ids = self.admin_ids.clone();

        // Renders the WHERE clause; `pg` selects `$n` placeholders, sqlite `?`.
        let render = |pg: bool| -> (String, Vec<ListBind>) {
            let mut counter = 0usize;
            let mut next = || -> String {
                counter += 1;
                if pg {
                    format!("${}", counter)
                } else {
                    "?".to_owned()
                }
            };
            let mut parts: Vec<String> = Vec::with_capacity(conds.len());
            let mut binds: Vec<ListBind> = Vec::new();
            for cond in &conds {
                match cond {
                    Cond::Keyset => match filter.sort {
                        UserListSort::Newest => {
                            let a = next();
                            let b = next();
                            let c = next();
                            binds.push(ListBind::Int(after_created));
                            binds.push(ListBind::Int(after_created));
                            binds.push(ListBind::Text(after_id.clone()));
                            parts.push(format!("(created_at < {a} OR (created_at = {b} AND id < {c}))"));
                        }
                        UserListSort::Oldest => {
                            let a = next();
                            let b = next();
                            let c = next();
                            binds.push(ListBind::Int(after_created));
                            binds.push(ListBind::Int(after_created));
                            binds.push(ListBind::Text(after_id.clone()));
                            parts.push(format!("(created_at > {a} OR (created_at = {b} AND id > {c}))"));
                        }
                        UserListSort::Username => {
                            let a = next();
                            let b = next();
                            let c = next();
                            binds.push(ListBind::Text(after_name.clone()));
                            binds.push(ListBind::Text(after_name.clone()));
                            binds.push(ListBind::Text(after_id.clone()));
                            parts.push(format!("(LOWER(username) > LOWER({a}) OR (LOWER(username) = LOWER({b}) AND id > {c}))"));
                        }
                    },
                    Cond::Query => {
                        let a = next();
                        let b = next();
                        binds.push(ListBind::Text(needle_like.clone()));
                        binds.push(ListBind::Text(needle_like.clone()));
                        parts.push(format!("(LOWER(username) LIKE {a} ESCAPE '\\' OR LOWER(id) LIKE {b} ESCAPE '\\')"));
                    }
                    Cond::Active => {
                        parts.push("(disabled = 0 AND banned = 0)".to_owned());
                    }
                    Cond::Disabled => {
                        parts.push("(disabled != 0)".to_owned());
                    }
                    Cond::Banned => {
                        parts.push("(banned != 0)".to_owned());
                    }
                    Cond::Admin => {
                        let holes: Vec<String> =
                            admin_ids.iter().map(|_| next()).collect();
                        for id in &admin_ids {
                            binds.push(ListBind::Text(id.clone()));
                        }
                        parts.push(format!("(id IN ({}))", holes.join(", ")));
                    }
                    Cond::Badge => {
                        // Placeholders and binds follow TEXT order (sqlite `?`
                        // is positional; Postgres `$n` just needs 1..=max
                        // bound, which text order also satisfies).
                        let like_custom = next();
                        binds.push(ListBind::Text(badge_like.clone()));
                        let like_admin = next();
                        binds.push(ListBind::Text(badge_like.clone()));
                        let mut admin_holes: Vec<String> = Vec::new();
                        for id in &admin_ids {
                            admin_holes.push(next());
                            binds.push(ListBind::Text(id.clone()));
                        }
                        let like_early = next();
                        binds.push(ListBind::Text(badge_like.clone()));
                        // Stored custom badges, plus the two computed names
                        // with their membership rule, mirroring `user_badges`.
                        let admin_part = if admin_holes.is_empty() {
                            "0 = 1".to_owned()
                        } else {
                            format!("id IN ({})", admin_holes.join(", "))
                        };
                        parts.push(format!(
                            "(LOWER(custom_badges_json) LIKE {like_custom} ESCAPE '\\' OR ('admin' LIKE {like_admin} ESCAPE '\\' AND ({admin_part})) OR ('early' LIKE {like_early} ESCAPE '\\' AND user_rank >= 1 AND user_rank <= 200))"
                        ));
                    }
                    Cond::From => {
                        let place = next();
                        binds.push(ListBind::Int(filter.created_after.unwrap_or(0)));
                        parts.push(format!("(created_at >= {place})"));
                    }
                    Cond::To => {
                        let place = next();
                        binds.push(ListBind::Int(filter.created_before.unwrap_or(0)));
                        parts.push(format!("(created_at <= {place})"));
                    }
                }
            }
            (parts.join(" AND "), binds)
        };

        // The rank window runs over the whole table (like search) so `early`
        // keeps its global meaning under any filter combination.
        const COLS: &str = "id, username, password_hash, recovery_hash, profile_json, status, disabled, banned, created_at, username_changes_json, custom_badges_json";
        let order = match filter.sort {
            UserListSort::Newest => "ORDER BY created_at DESC, id DESC",
            UserListSort::Oldest => "ORDER BY created_at ASC, id ASC",
            UserListSort::Username => "ORDER BY LOWER(username) ASC, id ASC",
        };
        let fetch = limit as i64 + 1;
        let from = "SELECT {COLS}, user_rank FROM (SELECT {COLS}, ROW_NUMBER() OVER (ORDER BY created_at ASC, id ASC) AS user_rank FROM users) ranked_users"
            .replace("{COLS}", COLS);
        let (raw_rows_len, users, skipped) = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let (where_sql, binds) = render(false);
                let sql = format!("{from} WHERE {where_sql} {order} LIMIT ?");
                let mut query = sqlx::query(&sql);
                bind_all!(query, binds);
                let rows = query
                    .bind(fetch)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("List users query", err))?;
                let raw_len = rows.len();
                let (users, skipped) = self.decode_stored_rows(rows);
                (raw_len, users, skipped)
            }
            SqlBackend::Postgres(pool) => {
                let (where_sql, binds) = render(true);
                let limit_slot = binds.len() + 1;
                let sql = format!("{from} WHERE {where_sql} {order} LIMIT ${limit_slot}");
                let mut query = sqlx::query(&sql);
                bind_all!(query, binds);
                let rows = query
                    .bind(fetch)
                    .fetch_all(pool)
                    .await
                    .map_err(|err| ApiError::internal("List users query", err))?;
                let raw_len = rows.len();
                let (users, skipped) = self.decode_stored_rows(rows);
                (raw_len, users, skipped)
            }
        };
        let has_more = raw_rows_len > limit;
        let mut public: Vec<PublicUser> = Vec::with_capacity(users.len().min(limit));
        for user in users.into_iter().take(limit) {
            public.push(self.public_user(user));
        }
        let next_cursor = if has_more {
            public.last().map(|u| match filter.sort {
                UserListSort::Username => format!("u:{}:{}", u.username, u.id),
                _ => format!("t:{}:{}", u.created_at, u.id),
            })
        } else {
            None
        };
        Ok((public, next_cursor, skipped))
    }

        pub async fn set_user_disabled(&self, user_id: &str, disabled: bool) -> ApiResult<()> {        let value = if disabled { 1i64 } else { 0i64 };
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

    pub async fn session_revalidation(
        &self,
        user_id: &str,
    ) -> ApiResult<Option<(bool, bool, bool, Vec<String>, String)>> {
        let Some(user) = self.user_by_id(user_id).await? else {
            return Ok(None);
        };
        if user.disabled || user.banned {
            return Ok(None);
        }
        let is_admin = self.is_admin(&user.id);
        let badges = self.user_badges(&user);
        Ok(Some((user.disabled, user.banned, is_admin, badges, user.username)))
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
            custom_badges: user.custom_badges,
            created_at: user.created_at,
        }
    }
}

/// Badges no admin can hand out: `admin` comes from the configured admin list,
/// `staff` and `system` mark the product's own people and its official
/// account. Any of them worn by a member would pass for staff, so only the
/// server grants and removes them. Mirrors `RESERVED_BADGE_IDS` in the client.
///
/// `early` is deliberately absent: the server grants it to the first accounts,
/// and an admin may hand it out afterwards.
const RESERVED_BADGES: &[&str] = &["admin", "staff", "system"];

fn sanitize_custom_badges(badges: &[String]) -> Vec<String> {
    let mut cleaned: Vec<String> = Vec::new();
    for badge in badges {
        let sanitized: String = badge
            .trim()
            .to_lowercase()
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
            .take(MAX_USER_BADGE_LEN)
            .collect();
        if sanitized.is_empty()
            || RESERVED_BADGES.contains(&sanitized.as_str())
            || cleaned.contains(&sanitized)
        {
            continue;
        }
        cleaned.push(sanitized);
        if cleaned.len() >= MAX_USER_BADGES {
            break;
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
            ensure_sqlite_database(&config.url, config.create_if_missing).await?;
            let options = SqliteConnectOptions::from_str(&config.url)
                .map_err(|err| ApiError::internal("Invalid SQLite room database URL", err))?
                .create_if_missing(config.create_if_missing);
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

    pub async fn create_room_if_absent(&self, room_id: &str, room: &RoomRecord) -> ApiResult<bool> {
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

        let created = match &self.backend {
            SqlBackend::Sqlite(pool) => {
                let result = sqlx::query(
                    "INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(room_id) DO NOTHING",
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
                .map_err(|err| ApiError::internal("Create room", err))?;
                result.rows_affected() == 1
            }
            SqlBackend::Postgres(pool) => {
                let result = sqlx::query(
                    "INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at, kind, description, owner_id, roles_json, bans_json, timeouts_json, chat_locked, mod_permissions_json, calls_enabled) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
                     ON CONFLICT(room_id) DO NOTHING",
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
                .map_err(|err| ApiError::internal("Create room", err))?;
                result.rows_affected() == 1
            }
        };
        Ok(created)
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

    /// Fully deletes a community room (owner). Returns true if a row existed.
    /// The caller purges RAM messages, sessions, and broadcasts the eviction.
    pub async fn delete_room(&self, room_id: &str) -> ApiResult<bool> {
        let deleted = match &self.backend {
            SqlBackend::Sqlite(pool) => sqlx::query("DELETE FROM rooms WHERE room_id = ?")
                .bind(room_id)
                .execute(pool)
                .await
                .map_err(|err| ApiError::internal("Delete room", err))?
                .rows_affected(),
            SqlBackend::Postgres(pool) => sqlx::query("DELETE FROM rooms WHERE room_id = $1")
                .bind(room_id)
                .execute(pool)
                .await
                .map_err(|err| ApiError::internal("Delete room", err))?
                .rows_affected(),
        };
        Ok(deleted > 0)
    }
}

/// File path designated by a SQLite URL, if any.
///
/// `None` for `sqlite::memory:` and for URLs that designate no file.
fn sqlite_file_path(url: &str) -> Option<PathBuf> {
    let raw = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let raw = raw.split('?').next().unwrap_or(raw).trim();
    if raw.is_empty() || raw == ":memory:" {
        return None;
    }
    Some(PathBuf::from(raw))
}

async fn ensure_sqlite_database(url: &str, create_if_missing: bool) -> ApiResult<()> {
    let Some(path) = sqlite_file_path(url) else {
        return Ok(());
    };

    if fs::try_exists(&path).await.unwrap_or(false) {
        tracing::info!("SQLite database: {}", path.display());
        return Ok(());
    }

    if !create_if_missing {
        return Err(ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "SQLite database file not found: {}. Refusing to start: opening an empty \
                 database would hide every account from users. \
                 Fix [database].url, or pin the deploy root with the \
                 QXP_ROOT environment variable, or — for a first deployment only — \
                 set [database].createIfMissing = true.",
                path.display()
            ),
        ));
    }

    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| ApiError::internal("Prepare sqlite directory", err))?;
    }
    tracing::warn!(
        "Creating an **empty** SQLite database (createIfMissing = true): {}",
        path.display()
    );
    Ok(())
}

/// Admin user-center tests: listing resilience at scale-shaped data (one bad
/// row must not fail the page), server-side filters, sort orders with stable
/// keyset pagination, and cursor formats. SQLite only — Postgres shares the
/// same query builder, and CI has no Postgres service.
#[cfg(test)]
mod admin_users_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DB_SEQ: AtomicU64 = AtomicU64::new(0);

    async fn test_db(admin_ids: Vec<String>) -> (AccountDatabase, PathBuf) {
        let n = TEST_DB_SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "qx-admin-list-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&path).await;
        let config = DatabaseConfig {
            kind: "sqlite".to_owned(),
            url: format!("sqlite://{}?mode=rwc", path.display()),
            create_if_missing: true,
        };
        let db = AccountDatabase::connect(&config, admin_ids, true)
            .await
            .expect("test database connects");
        (db, path)
    }

    async fn seed(db: &AccountDatabase, username: &str) -> String {
        db.register(username, "test-password-123")
            .await
            .expect("register test user")
            .0
            .id
    }

    async fn set_created_at(db: &AccountDatabase, user_id: &str, created_at: i64) {
        match &db.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET created_at = ? WHERE id = ?")
                    .bind(created_at)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .expect("set created_at");
            }
            SqlBackend::Postgres(_) => unreachable!("sqlite-only test"),
        }
    }

    /// Legacy/hand-edited type mix: TEXT where the schema says BIGINT.
    async fn corrupt_disabled_flag(db: &AccountDatabase, user_id: &str) {
        match &db.backend {
            SqlBackend::Sqlite(pool) => {
                sqlx::query("UPDATE users SET disabled = 'yes' WHERE id = ?")
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .expect("corrupt row");
            }
            SqlBackend::Postgres(_) => unreachable!("sqlite-only test"),
        }
    }

    fn plain_filter() -> UserListFilter {
        UserListFilter::default()
    }

    fn usernames(users: &[PublicUser]) -> Vec<String> {
        users.iter().map(|u| u.username.clone()).collect()
    }

    #[tokio::test]
    async fn list_skips_malformed_row_instead_of_500() {
        let (db, path) = test_db(vec![]).await;
        let keep_a = seed(&db, "list-skip-anna").await;
        let broken = seed(&db, "list-skip-boris").await;
        let keep_c = seed(&db, "list-skip-cleo").await;
        corrupt_disabled_flag(&db, &broken).await;

        let (users, next, skipped) = db
            .list_users_page(&plain_filter(), 100, None)
            .await
            .expect("listing survives one malformed row");
        assert_eq!(skipped, 1, "the corrupt row is reported, not fatal");
        let ids: Vec<String> = users.iter().map(|u| u.id.clone()).collect();
        assert!(ids.contains(&keep_a) && ids.contains(&keep_c));
        assert!(!ids.contains(&broken));
        // Two good rows under the limit: no next page.
        assert!(next.is_none());
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn search_skips_malformed_row() {
        let (db, path) = test_db(vec![]).await;
        seed(&db, "search-skip-anna").await;
        let broken = seed(&db, "search-skip-boris").await;
        corrupt_disabled_flag(&db, &broken).await;

        let (users, skipped) = db.search_users("search-skip", 30).await.expect("search works");
        assert_eq!(skipped, 1);
        assert_eq!(usernames(&users), vec!["search-skip-anna".to_owned()]);
        let _ = tokio::fs::remove_file(&path).await;
    }

    /// alice: admin + vip badge, created 1000. bob: disabled, 2000.
    /// carol: banned, 3000. dave: plain, 4000. Returns alice's id.
    async fn seed_filtered(db: &AccountDatabase) -> String {
        let alice = seed(db, "filter-alice").await;
        let bob = seed(db, "filter-boris").await;
        let carol = seed(db, "filter-cleo").await;
        let dave = seed(db, "filter-dario").await;
        db.set_user_badges(&alice, &["vip".to_owned()])
            .await
            .expect("grant badge");
        db.set_user_disabled(&bob, true).await.expect("disable");
        db.set_user_banned(&carol, true).await.expect("ban");
        set_created_at(db, &alice, 1000).await;
        set_created_at(db, &bob, 2000).await;
        set_created_at(db, &carol, 3000).await;
        set_created_at(db, &dave, 4000).await;
        alice
    }

    fn filtered(status: UserListStatus) -> UserListFilter {
        UserListFilter {
            status,
            ..UserListFilter::default()
        }
    }

    #[tokio::test]
    async fn list_status_filters() {
        let (db, path) = test_db(vec![]).await;
        seed_filtered(&db).await;

        let (users, _, _) = db
            .list_users_page(&filtered(UserListStatus::Disabled), 100, None)
            .await
            .expect("disabled filter");
        assert_eq!(usernames(&users), vec!["filter-boris".to_owned()]);

        let (users, _, _) = db
            .list_users_page(&filtered(UserListStatus::Banned), 100, None)
            .await
            .expect("banned filter");
        assert_eq!(usernames(&users), vec!["filter-cleo".to_owned()]);

        let (users, _, _) = db
            .list_users_page(&filtered(UserListStatus::Active), 100, None)
            .await
            .expect("active filter");
        // Default sort is newest-first.
        assert_eq!(usernames(&users), vec!["filter-dario".to_owned(), "filter-alice".to_owned()]);

        // `admin` with nobody configured matches nothing, without error.
        let (users, next, _) = db
            .list_users_page(&filtered(UserListStatus::Admin), 100, None)
            .await
            .expect("admin filter, empty admin list");
        assert!(users.is_empty() && next.is_none());
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn list_admin_status_uses_configured_ids() {
        let (mut db, path) = test_db(vec![]).await;
        let alice_id = seed_filtered(&db).await;
        db.admin_ids.push(alice_id);

        let (users, _, _) = db
            .list_users_page(&filtered(UserListStatus::Admin), 100, None)
            .await
            .expect("admin filter");
        assert_eq!(usernames(&users), vec!["filter-alice".to_owned()]);

        // The computed `admin` badge name resolves through the same list.
        let badge = UserListFilter {
            badge: "admin".to_owned(),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&badge, 100, None).await.expect("badge admin");
        assert_eq!(usernames(&users), vec!["filter-alice".to_owned()]);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn list_query_badge_and_date_filters() {
        let (db, path) = test_db(vec![]).await;
        seed_filtered(&db).await;

        let q = UserListFilter {
            query: "cleo".to_owned(),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&q, 100, None).await.expect("q filter");
        assert_eq!(usernames(&users), vec!["filter-cleo".to_owned()]);

        // Username OR id substring, case-insensitive.
        let q = UserListFilter {
            query: "FILTER-".to_owned(),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&q, 100, None).await.expect("q case");
        assert_eq!(users.len(), 4);

        let badge = UserListFilter {
            badge: "vip".to_owned(),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&badge, 100, None).await.expect("badge");
        assert_eq!(usernames(&users), vec!["filter-alice".to_owned()]);

        // Computed `early` badge: every seeded account ranks in the top 200.
        let early = UserListFilter {
            badge: "early".to_owned(),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&early, 100, None).await.expect("early");
        assert_eq!(users.len(), 4);

        let from = UserListFilter {
            created_after: Some(2500),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&from, 100, None).await.expect("from");
        assert_eq!(usernames(&users), vec!["filter-dario".to_owned(), "filter-cleo".to_owned()]);

        let range = UserListFilter {
            created_after: Some(2000),
            created_before: Some(3000),
            ..UserListFilter::default()
        };
        let (users, _, _) = db.list_users_page(&range, 100, None).await.expect("range");
        assert_eq!(usernames(&users), vec!["filter-cleo".to_owned(), "filter-boris".to_owned()]);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn list_pagination_chains_to_exhaustion_in_every_sort() {
        let (db, path) = test_db(vec![]).await;
        for (i, name) in ["page-anna", "page-boris", "page-cleo", "page-dario", "page-elio"]
            .iter()
            .enumerate()
        {
            let id = seed(&db, name).await;
            set_created_at(&db, &id, 1000 * (i as i64 + 1)).await;
        }

        for sort in [UserListSort::Newest, UserListSort::Oldest, UserListSort::Username] {
            let filter = UserListFilter {
                sort,
                ..UserListFilter::default()
            };
            let mut seen: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut pages = 0;
            loop {
                let (users, next, skipped) = db
                    .list_users_page(&filter, 2, cursor.as_deref())
                    .await
                    .expect("page fetches");
                assert_eq!(skipped, 0);
                pages += 1;
                assert!(pages < 10, "pagination must terminate");
                seen.extend(users.iter().map(|u| u.username.clone()));
                cursor = next;
                if cursor.is_none() {
                    break;
                }
            }
            let mut sorted = seen.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(seen.len(), 5, "every account exactly once ({sort:?})");
            assert_eq!(sorted.len(), 5, "no duplicates ({sort:?})");
            // Order matches the requested sort.
            match sort {
                UserListSort::Newest => assert_eq!(
                    seen,
                    vec!["page-elio".to_owned(), "page-dario".to_owned(), "page-cleo".to_owned(), "page-boris".to_owned(), "page-anna".to_owned()]
                ),
                UserListSort::Oldest => assert_eq!(
                    seen,
                    vec!["page-anna".to_owned(), "page-boris".to_owned(), "page-cleo".to_owned(), "page-dario".to_owned(), "page-elio".to_owned()]
                ),
                UserListSort::Username => assert_eq!(
                    seen,
                    vec!["page-anna".to_owned(), "page-boris".to_owned(), "page-cleo".to_owned(), "page-dario".to_owned(), "page-elio".to_owned()]
                ),
            }
        }
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn list_accepts_legacy_cursor_and_rejects_garbage() {
        let (db, path) = test_db(vec![]).await;
        // Distinct timestamps: no id-tiebreak involved, deterministic order.
        for (i, name) in ["cursor-anna", "cursor-boris", "cursor-cleo"]
            .iter()
            .enumerate()
        {
            let id = seed(&db, name).await;
            set_created_at(&db, &id, 1000 * (i as i64 + 1)).await;
        }
        let oldest = UserListFilter {
            sort: UserListSort::Oldest,
            ..UserListFilter::default()
        };
        let (page1, next1, _) = db.list_users_page(&oldest, 2, None).await.expect("p1");
        assert_eq!(page1.len(), 2);
        let cursor = next1.expect("has next");
        assert!(cursor.starts_with("t:"), "tagged cursor, got {cursor}");
        // Legacy untagged shape still parses.
        let legacy = cursor.strip_prefix("t:").expect("tag").to_owned();
        let (page2a, _, _) = db
            .list_users_page(&oldest, 2, Some(&cursor))
            .await
            .expect("tagged cursor");
        let (page2b, _, _) = db
            .list_users_page(&oldest, 2, Some(&legacy))
            .await
            .expect("legacy cursor");
        assert_eq!(usernames(&page2a), usernames(&page2b));

        // A time cursor is a plain position: it stays valid when flipping
        // between the two time orders. A username cursor never is.
        let newest = UserListFilter {
            sort: UserListSort::Newest,
            ..UserListFilter::default()
        };
        let (flip_page, _, _) = db
            .list_users_page(&newest, 2, Some(&cursor))
            .await
            .expect("time cursor works in both directions");
        assert_eq!(usernames(&flip_page), vec!["cursor-anna".to_owned()]);

        let username_sort = UserListFilter {
            sort: UserListSort::Username,
            ..UserListFilter::default()
        };
        let (_, name_cursor, _) = db
            .list_users_page(&username_sort, 2, None)
            .await
            .expect("username first page");
        let name_cursor = name_cursor.expect("username has next");
        assert!(name_cursor.starts_with("u:"), "tagged cursor, got {name_cursor}");
        assert!(db
            .list_users_page(&oldest, 2, Some(&name_cursor))
            .await
            .is_err());
        assert!(db.list_users_page(&oldest, 2, Some("nonsense")).await.is_err());
        assert!(db.list_users_page(&oldest, 2, Some("t:abc:123")).await.is_err());
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn filter_params_reject_garbage() {
        assert!(UserListFilter::from_params(None, None, None, None, None, None).is_ok());
        assert!(UserListFilter::from_params(None, Some("nope"), None, None, None, None).is_err());
        assert!(UserListFilter::from_params(None, None, None, None, None, Some("nope")).is_err());
        assert!(UserListFilter::from_params(None, None, None, Some("abc"), None, None).is_err());
        assert!(UserListFilter::from_params(None, None, None, Some("-5"), None, None).is_err());
        assert!(
            UserListFilter::from_params(None, None, None, Some("3000"), Some("1000"), None).is_err()
        );
        let ok =
            UserListFilter::from_params(Some(" BoB "), Some("DISABLED"), Some("VIP"), Some("1000"), Some("2000"), Some("username"))
                .expect("valid params");
        assert_eq!(ok.query, "BoB");
        assert_eq!(ok.status, UserListStatus::Disabled);
        assert_eq!(ok.badge, "vip");
        assert_eq!(ok.created_after, Some(1000));
        assert_eq!(ok.created_before, Some(2000));
        assert_eq!(ok.sort, UserListSort::Username);
    }
}
