use std::path::PathBuf;

use serde::Deserialize;
use tokio::fs;
use tracing::{error, warn};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api: ApiConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub domain: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, rename = "adminPassword")]
    pub admin_password: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            domain: String::new(),
            port: default_port(),
            admin_password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_heartbeat_interval", rename = "heartbeatInterval")]
    pub heartbeat_interval: u64,
    #[serde(default = "default_max_connections", rename = "maxConnectionsPerIp")]
    pub max_connections_per_ip: usize,
    #[serde(default, rename = "latestVersion")]
    pub latest_version: Option<String>,
    #[serde(default = "default_public_dir", rename = "publicDir")]
    pub public_dir: String,
    #[serde(default = "default_webchat_index", rename = "webchatIndex")]
    pub webchat_index: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: default_heartbeat_interval(),
            max_connections_per_ip: default_max_connections(),
            latest_version: None,
            public_dir: default_public_dir(),
            webchat_index: default_webchat_index(),
        }
    }
}

fn default_port() -> u16 {
    4560
}

fn default_heartbeat_interval() -> u64 {
    3_000
}

fn default_max_connections() -> usize {
    3
}

fn default_public_dir() -> String {
    "serve.public".to_owned()
}

fn default_webchat_index() -> String {
    "index.html".to_owned()
}


pub fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

pub async fn load_config() -> Config {
    let env = if std::env::var("PRODUCTION").is_ok() {
        "prod"
    } else {
        "dev"
    };

    let default_path = PathBuf::from(format!("files/config.{}.toml", env));
    let custom_path = PathBuf::from("files/config.custom.toml");
    let config_path = if custom_path.exists() {
        custom_path
    } else {
        default_path
    };

    match fs::read_to_string(&config_path).await {
        Ok(raw) => match toml::from_str::<Config>(&raw) {
            Ok(config) => config,
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
    match fs::read_to_string("src/blocklist.json").await {
        Ok(contents) => serde_json::from_str::<Vec<String>>(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}
