use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::fs;
use tracing::{error, warn};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub rtc: RtcConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub security: SecurityConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            network: NetworkConfig::default(),
            rtc: RtcConfig::default(),
            database: DatabaseConfig::default(),
            security: SecurityConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub domain: String,
    #[serde(default, rename = "publicDomain")]
    pub public_domain: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            domain: String::new(),
            public_domain: String::new(),
            port: default_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_heartbeat_interval", rename = "heartbeatInterval")]
    pub heartbeat_interval: u64,
    #[serde(default, rename = "latestVersion")]
    pub latest_version: Option<String>,
    #[serde(default = "default_public_dir", rename = "publicDir")]
    pub public_dir: String,
    #[serde(default = "default_webchat_index", rename = "webchatIndex")]
    pub webchat_index: String,
    #[serde(default = "default_upload_dir", rename = "uploadDir")]
    pub upload_dir: String,
    #[serde(default = "default_upload_public_base", rename = "uploadPublicBase")]
    pub upload_public_base: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: default_heartbeat_interval(),
            latest_version: None,
            public_dir: default_public_dir(),
            webchat_index: default_webchat_index(),
            upload_dir: default_upload_dir(),
            upload_public_base: default_upload_public_base(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RtcConfig {
    #[serde(default = "default_relay_only", rename = "relayOnly")]
    pub relay_only: bool,
    #[serde(default, rename = "turnUrls")]
    pub turn_urls: Vec<String>,
    #[serde(default, rename = "turnUsername")]
    pub turn_username: String,
    #[serde(default, rename = "turnCredential")]
    pub turn_credential: String,
}

impl Default for RtcConfig {
    fn default() -> Self {
        Self {
            relay_only: default_relay_only(),
            turn_urls: Vec::new(),
            turn_username: String::new(),
            turn_credential: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_kind")]
    pub kind: String,
    #[serde(default = "default_database_url")]
    pub url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            kind: default_database_kind(),
            url: default_database_url(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    #[serde(default, rename = "adminIds")]
    pub admin_ids: Vec<String>,
    #[serde(default = "default_register_enabled", rename = "registerEnabled")]
    pub register_enabled: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            admin_ids: Vec::new(),
            register_enabled: default_register_enabled(),
        }
    }
}

fn default_port() -> u16 {
    4560
}

fn default_heartbeat_interval() -> u64 {
    3_000
}

fn default_public_dir() -> String {
    "web/dist".to_owned()
}

fn default_webchat_index() -> String {
    "index.html".to_owned()
}

fn default_upload_dir() -> String {
    "files/uploads".to_owned()
}

fn default_upload_public_base() -> String {
    "/app/uploads".to_owned()
}

fn default_relay_only() -> bool {
    true
}

fn default_database_kind() -> String {
    "sqlite".to_owned()
}

fn default_database_url() -> String {
    "sqlite://files/qxp.sqlite".to_owned()
}

fn default_register_enabled() -> bool {
    true
}

pub fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn resolve_project_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root().join(path)
    }
}

pub async fn load_config() -> Config {
    let env = if std::env::var("PRODUCTION").is_ok() {
        "prod"
    } else {
        "dev"
    };

    let default_path = resolve_project_path(format!("files/config.{}.toml", env));
    let custom_path = resolve_project_path("files/config.custom.toml");
    let config_path = if custom_path.exists() {
        custom_path
    } else {
        default_path
    };

    match fs::read_to_string(&config_path).await {
        Ok(raw) => match toml::from_str::<Config>(&raw) {
            Ok(mut config) => {
                config.network.public_dir = resolve_project_path(&config.network.public_dir)
                    .to_string_lossy()
                    .into_owned();
                config.network.upload_dir = resolve_project_path(&config.network.upload_dir)
                    .to_string_lossy()
                    .into_owned();
                if let Some(path) = config.database.url.strip_prefix("sqlite://") {
                    if !path.is_empty() && !Path::new(path).is_absolute() {
                        config.database.url = format!(
                            "sqlite://{}",
                            resolve_project_path(path).to_string_lossy()
                        );
                    }
                }
                config
            }
            Err(err) => {
                error!("Failed to parse {}: {}", config_path.display(), err);
                Config::default()
            }
        },
        Err(err) => {
            warn!("Failed to read {}: {}", config_path.display(), err);
            Config::default()
        }
    }
}

pub async fn load_blocklist_terms() -> Vec<String> {
    let path = resolve_project_path("rust/src/blocklist.json");
    match fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str::<Vec<String>>(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}
