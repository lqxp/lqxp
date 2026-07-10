use std::{collections::HashMap, path::Path, str::FromStr};

use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use tokio::fs;

use crate::{
    config::DatabaseConfig,
    models::{RoomIcon, RoomRecord},
};

pub type AppResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct RoomDatabase {
    pool: SqlitePool,
}

impl RoomDatabase {
    pub async fn connect(config: &DatabaseConfig) -> AppResult<Self> {
        prepare_sqlite_path(&config.url).await?;
        let options = SqliteConnectOptions::from_str(&config.url)?.create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> AppResult<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS rooms (
                room_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                icon_json TEXT,
                members_json TEXT NOT NULL DEFAULT '[]',
                updated_at BIGINT NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn room_records(&self) -> HashMap<String, RoomRecord> {
        let rows = sqlx::query("SELECT room_id, title, icon_json, members_json FROM rooms")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        rows.into_iter()
            .filter_map(|row| {
                let room_id = row.try_get::<String, _>("room_id").ok()?;
                let title = row
                    .try_get::<String, _>("title")
                    .unwrap_or_else(|_| room_id.clone());
                let icon = row
                    .try_get::<Option<String>, _>("icon_json")
                    .ok()
                    .flatten()
                    .and_then(|value| serde_json::from_str::<RoomIcon>(&value).ok());
                let members = row
                    .try_get::<String, _>("members_json")
                    .ok()
                    .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                    .unwrap_or_default();

                Some((
                    room_id.clone(),
                    RoomRecord {
                        room_id,
                        title,
                        icon,
                        members,
                    },
                ))
            })
            .collect()
    }

    pub async fn room_record(&self, room_id: &str) -> Option<RoomRecord> {
        let row = sqlx::query(
            "SELECT room_id, title, icon_json, members_json FROM rooms WHERE room_id = ?",
        )
        .bind(room_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;

        let stored_room_id = row.try_get::<String, _>("room_id").ok()?;
        let title = row
            .try_get::<String, _>("title")
            .unwrap_or_else(|_| stored_room_id.clone());
        let icon = row
            .try_get::<Option<String>, _>("icon_json")
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str::<RoomIcon>(&value).ok());
        let members = row
            .try_get::<String, _>("members_json")
            .ok()
            .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
            .unwrap_or_default();

        Some(RoomRecord {
            room_id: stored_room_id,
            title,
            icon,
            members,
        })
    }

    pub async fn set_room_record(&self, room_id: &str, room: &RoomRecord) -> AppResult<()> {
        let icon_json = room
            .icon
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let members_json = serde_json::to_string(&room.members)?;
        sqlx::query(
            "INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(room_id) DO UPDATE SET title = excluded.title, icon_json = excluded.icon_json, members_json = excluded.members_json, updated_at = excluded.updated_at",
        )
        .bind(room_id)
        .bind(&room.title)
        .bind(icon_json)
        .bind(members_json)
        .bind(crate::utils::now_ms() as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn room_icon(&self, room_id: &str) -> Option<RoomIcon> {
        self.room_record(room_id).await.and_then(|room| room.icon)
    }

    pub async fn set_room_icon(&self, room_id: &str, icon: &RoomIcon) -> AppResult<()> {
        let mut room = self.room_record(room_id).await.unwrap_or(RoomRecord {
            room_id: room_id.to_owned(),
            title: room_id.to_owned(),
            icon: None,
            members: Vec::new(),
        });
        room.icon = Some(icon.clone());
        self.set_room_record(room_id, &room).await
    }
}

async fn prepare_sqlite_path(url: &str) -> AppResult<()> {
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
        fs::create_dir_all(parent).await?;
    }
    Ok(())
}
