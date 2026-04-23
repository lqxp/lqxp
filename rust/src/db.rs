use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{fs, sync::RwLock};
use tracing::warn;

use crate::models::BlacklistEntry;

pub type AppResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Default, Serialize, Deserialize)]
struct RawDatabase {
    #[serde(default)]
    json: Vec<DatabaseRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DatabaseRow {
    id: String,
    value: Value,
}

#[derive(Debug)]
pub struct JsonDatabase {
    path: PathBuf,
    inner: RwLock<HashMap<String, Value>>,
}

impl JsonDatabase {
    pub async fn load(path: PathBuf) -> Self {
        let mut data = HashMap::new();

        if let Ok(contents) = fs::read_to_string(&path).await {
            match serde_json::from_str::<RawDatabase>(&contents) {
                Ok(raw) => {
                    for row in raw.json {
                        data.insert(row.id, row.value);
                    }
                }
                Err(err) => {
                    warn!("Database malformed at {}: {}", path.display(), err);
                }
            }
        }

        Self {
            path,
            inner: RwLock::new(data),
        }
    }

    pub async fn get_value(&self, key: &str) -> Option<Value> {
        let store = self.inner.read().await;
        store.get(key).cloned()
    }

    pub async fn set_value(&self, key: &str, value: Value) -> AppResult<()> {
        {
            let mut store = self.inner.write().await;
            store.insert(key.to_owned(), value);
        }

        self.flush().await
    }

    pub async fn unique_push(&self, key: &str, item: Value) -> AppResult<()> {
        let mut current = self
            .get_value(key)
            .await
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();

        if !current.iter().any(|existing| existing == &item) {
            current.push(item);
            self.set_value(key, Value::Array(current)).await?;
        }

        Ok(())
    }

    async fn flush(&self) -> AppResult<()> {
        let parent = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        fs::create_dir_all(&parent).await?;

        let rows = {
            let store = self.inner.read().await;
            let mut rows: Vec<DatabaseRow> = store
                .iter()
                .map(|(id, value)| DatabaseRow {
                    id: id.clone(),
                    value: value.clone(),
                })
                .collect();
            rows.sort_by(|a, b| a.id.cmp(&b.id));
            rows
        };

        let payload = RawDatabase { json: rows };
        let encoded = serde_json::to_string_pretty(&payload)?;
        fs::write(&self.path, encoded).await?;
        Ok(())
    }

    pub async fn blacklisted_ips(&self) -> Vec<BlacklistEntry> {
        self.get_value("blacklisted_ips")
            .await
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    pub async fn set_blacklisted_ips(&self, entries: &[BlacklistEntry]) -> AppResult<()> {
        self.set_value("blacklisted_ips", serde_json::to_value(entries)?)
            .await
    }

    pub async fn logged_ips(&self) -> Vec<Value> {
        self.get_value("logged_ips")
            .await
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
    }
}
