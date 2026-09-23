use serde::Deserialize;
use std::{
    fmt,
    path::{Component, Path, PathBuf},
};
use tokio::fs;
use tracing::{info, warn};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
    pub web: WebConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    pub repo: String,
    pub directory: String,
    pub fresh: bool,
    #[serde(default)]
    pub tag: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            repo: "https://github.com/lqxp/client.git".to_owned(),
            directory: "web".to_owned(),
            fresh: true,
            tag: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebRevision {
    Fresh,
    Tag(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl WebConfig {
    pub fn revision(&self) -> Result<WebRevision, ConfigError> {
        validate_web_repo(&self.repo)?;
        match (self.fresh, self.tag.trim()) {
            (true, "") => Ok(WebRevision::Fresh),
            (false, tag) if !tag.is_empty() => Ok(WebRevision::Tag(tag.to_owned())),
            (true, _) => Err(ConfigError::new(
                "[web].tag must be empty when [web].fresh is true",
            )),
            (false, _) => Err(ConfigError::new(
                "[web].tag is required when [web].fresh is false",
            )),
        }
    }

    pub fn checkout_path(&self, root: &Path) -> Result<PathBuf, ConfigError> {
        let relative = Path::new(&self.directory);
        if relative.as_os_str().is_empty() || relative.is_absolute() {
            return Err(ConfigError::new(
                "[web].directory must be a non-empty relative path",
            ));
        }

        let canonical_root = std::fs::canonicalize(root).map_err(|err| {
            ConfigError::new(format!("cannot resolve QXP_ROOT {}: {err}", root.display()))
        })?;
        let boundary = canonical_root.parent().ok_or_else(|| {
            ConfigError::new("QXP_ROOT has no parent available for web checkout validation")
        })?;
        let mut target = canonical_root.clone();
        for component in relative.components() {
            match component {
                Component::Normal(part) => target.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !target.pop() {
                        return Err(ConfigError::new(
                            "[web].directory cannot traverse beyond the filesystem root",
                        ));
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ConfigError::new("[web].directory must be a relative path"));
                }
            }
        }

        if !target.starts_with(boundary)
            || target == boundary
            || canonical_root.starts_with(&target)
        {
            return Err(ConfigError::new(
                "[web].directory may target QXP_ROOT or one of its siblings, but not a parent",
            ));
        }

        const PROTECTED: &[&str] = &[
            ".codex",
            ".dockerignore",
            ".git",
            ".gitattributes",
            ".github",
            ".gitignore",
            ".superpowers",
            "Cargo.lock",
            "Cargo.toml",
            "Dockerfile",
            "LICENSE",
            "README.md",
            "deploy",
            "docker-compose.yml",
            "docs",
            "explain",
            "files",
            "nginx.conf",
            "placeholder.xcf",
            "pm2.config.cjs",
            "rust",
            "scripts",
            "serve.public",
            "target",
            "update.sh",
        ];
        if let Ok(within_root) = target.strip_prefix(&canonical_root) {
            if let Some(Component::Normal(first)) = within_root.components().next() {
                let first = first.to_string_lossy();
                if PROTECTED.contains(&first.as_ref()) {
                    return Err(ConfigError::new(format!(
                        "[web].directory cannot target protected path {first}"
                    )));
                }
            }
        }

        let relative_target = target.strip_prefix(boundary).map_err(|_| {
            ConfigError::new("[web].directory escapes the allowed checkout boundary")
        })?;
        let mut current = boundary.to_path_buf();
        for component in relative_target.components() {
            current.push(component.as_os_str());
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(ConfigError::new(format!(
                        "[web].directory crosses symlink {}",
                        current.display()
                    )));
                }
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(ConfigError::new(format!(
                        "cannot inspect web path {}: {err}",
                        current.display()
                    )));
                }
            }
        }

        Ok(target)
    }
}

fn validate_web_repo(repo: &str) -> Result<(), ConfigError> {
    let path = if let Some(path) = repo.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = repo.strip_prefix("git@github.com:") {
        path
    } else {
        return Err(ConfigError::new(
            "[web].repo must use GitHub HTTPS or SSH syntax",
        ));
    };

    let mut parts = path.split('/');
    let owner = parts.next().unwrap_or_default();
    let repository = parts.next().unwrap_or_default();
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    if parts.next().is_some()
        || !valid_part(owner)
        || !repository.ends_with(".git")
        || !valid_part(repository.trim_end_matches(".git"))
    {
        return Err(ConfigError::new(
            "[web].repo must identify OWNER/REPOSITORY.git on GitHub",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    #[serde(default)]
    pub domain: String,
    #[serde(default, rename = "publicDomain")]
    pub public_domain: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default, rename = "adminPassword")]
    pub admin_password_deprecated: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            domain: String::new(),
            public_domain: String::new(),
            port: default_port(),
            admin_password_deprecated: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct TurnServer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default, rename = "turnUrls")]
    pub urls: Vec<String>,
    #[serde(default, rename = "turnUsername")]
    pub username: String,
    #[serde(default, rename = "turnCredential")]
    pub credential: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtcConfig {
    #[serde(default = "default_relay_only", rename = "relayOnly")]
    pub relay_only: bool,
    // Legacy flat fields – auto-promoted into a single "default" server.
    #[serde(default, rename = "turnUrls")]
    pub turn_urls: Vec<String>,
    #[serde(default, rename = "turnUsername")]
    pub turn_username: String,
    #[serde(default, rename = "turnCredential")]
    pub turn_credential: String,
    // New multi-server config.
    #[serde(default, rename = "servers")]
    pub servers: Vec<TurnServer>,
    #[serde(default, rename = "defaultTurnServer")]
    pub default_turn_server: String,
}

impl RtcConfig {
    /// Returns the resolved list of TURN servers: explicit `[[rtc.servers]]`
    /// entries first, then the legacy flat entry (if any) as a fallback.
    pub fn resolved_servers(&self) -> Vec<TurnServer> {
        let mut list: Vec<TurnServer> = self.servers.clone();

        // Auto-promote legacy flat config into a single server entry.
        if !self.turn_urls.is_empty() {
            let already_has = list.iter().any(|s| s.urls == self.turn_urls);
            if !already_has {
                list.push(TurnServer {
                    id: "legacy".into(),
                    label: "Serveur TURN".into(),
                    hint: String::new(),
                    urls: self.turn_urls.clone(),
                    username: self.turn_username.clone(),
                    credential: self.turn_credential.clone(),
                });
            }
        }

        // Ensure every server has a non-empty id (derive from label otherwise).
        let mut seen = std::collections::HashSet::new();
        for (i, server) in list.iter_mut().enumerate() {
            if server.id.is_empty() {
                server.id = if server.label.is_empty() {
                    format!("server-{}", i)
                } else {
                    server.label.to_lowercase().replace(' ', "-")
                };
            }
            seen.insert(server.id.clone());
        }

        list
    }
}

impl Default for RtcConfig {
    fn default() -> Self {
        Self {
            relay_only: default_relay_only(),
            turn_urls: Vec::new(),
            turn_username: String::new(),
            turn_credential: String::new(),
            servers: Vec::new(),
            default_turn_server: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    #[serde(default = "default_database_kind")]
    pub kind: String,
    #[serde(default = "default_database_url")]
    pub url: String,
    #[serde(default, rename = "createIfMissing")]
    pub create_if_missing: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            kind: default_database_kind(),
            url: default_database_url(),
            create_if_missing: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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

pub fn project_root() -> PathBuf {
    if let Some(root) = std::env::var_os("QXP_ROOT") {
        let root = PathBuf::from(root);
        if !root.as_os_str().is_empty() {
            return root;
        }
    }
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

pub async fn load_config() -> Result<Config, ConfigError> {
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
                config.web.revision()?;
                if !config.api.admin_password_deprecated.trim().is_empty() {
                    warn!(
                        "Config {} sets [api].adminPassword, which is ignored (use [security].adminIds). Remove it.",
                        config_path.display()
                    );
                }
                config.network.public_dir = resolve_project_path(&config.network.public_dir)
                    .to_string_lossy()
                    .into_owned();
                config.network.upload_dir = resolve_project_path(&config.network.upload_dir)
                    .to_string_lossy()
                    .into_owned();
                if let Some(path) = config.database.url.strip_prefix("sqlite://") {
                    if !path.is_empty() && !Path::new(path).is_absolute() {
                        config.database.url =
                            format!("sqlite://{}", resolve_project_path(path).to_string_lossy());
                    }
                }

                info!(
                    "Configuration: {} (racine: {}, PRODUCTION: {})",
                    config_path.display(),
                    project_root().display(),
                    std::env::var("PRODUCTION").is_ok()
                );
                if config.database.url.starts_with("sqlite") {
                    info!("Base de données: {}", config.database.url);
                }
                if config.database.create_if_missing {
                    warn!(
                        "createIfMissing = true : si le fichier est absent, une base **vide** \
                         sera créée à {}. À réserver au premier déploiement.",
                        config.database.url
                    );
                }
                Ok(config)
            }
            Err(err) => Err(ConfigError::new(format!(
                "configuration {} illisible: {err}",
                config_path.display()
            ))),
        },
        Err(err) => Err(ConfigError::new(format!(
            "configuration {} introuvable ou illisible: {err}",
            config_path.display()
        ))),
    }
}

pub async fn load_blocklist_terms() -> Vec<String> {
    let path = resolve_project_path("rust/src/blocklist.json");
    match fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str::<Vec<String>>(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lqxp-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temporary root");
        path
    }

    fn web_config(directory: &str) -> WebConfig {
        WebConfig {
            repo: "https://github.com/lqxp/client.git".into(),
            directory: directory.into(),
            fresh: true,
            tag: String::new(),
        }
    }

    #[test]
    fn web_mode_requires_exactly_fresh_or_tag() {
        let cases = [
            (true, "", true),
            (false, "v1.20.5", true),
            (true, "v1.20.5", false),
            (false, "", false),
        ];
        for (fresh, tag, valid) in cases {
            let web = WebConfig {
                repo: "https://github.com/lqxp/client.git".into(),
                directory: "web".into(),
                fresh,
                tag: tag.into(),
            };
            assert_eq!(web.revision().is_ok(), valid);
        }
    }

    #[test]
    fn web_repo_accepts_only_github_https_and_ssh_conventions() {
        for repo in [
            "https://github.com/lqxp/client.git",
            "git@github.com:lqxp/client.git",
        ] {
            assert!(validate_web_repo(repo).is_ok(), "{repo}");
        }
        for repo in [
            "https://token@github.com/lqxp/client.git",
            "https://gitlab.com/lqxp/client.git",
            "file:///tmp/client.git",
            "git@github.com:lqxp/client",
        ] {
            assert!(validate_web_repo(repo).is_err(), "{repo}");
        }
    }

    #[test]
    fn checkout_path_allows_a_sibling_but_not_a_parent_or_absolute_path() {
        let workspace = temporary_root("checkout-paths");
        let root = workspace.join("lqxp");
        std::fs::create_dir(&root).expect("create project root");
        assert_eq!(
            web_config("web").checkout_path(&root).unwrap(),
            root.join("web")
        );
        assert_eq!(
            web_config("generated/web").checkout_path(&root).unwrap(),
            root.join("generated/web")
        );
        assert_eq!(
            web_config("../web").checkout_path(&root).unwrap(),
            workspace.join("web")
        );
        assert_eq!(
            web_config("../client/web").checkout_path(&root).unwrap(),
            workspace.join("client/web")
        );
        for unsafe_path in [
            ".",
            "..",
            "../lqxp",
            "../../web",
            "/tmp/web",
            ".git",
            "files/web",
            "rust/web",
            "target/web",
            "docs/web",
            "deploy/web",
            "scripts/web",
            ".github/web",
            "Cargo.toml",
            "README.md",
            "Dockerfile",
            "update.sh",
        ] {
            assert!(
                web_config(unsafe_path).checkout_path(&root).is_err(),
                "{unsafe_path}"
            );
        }
        std::fs::remove_dir_all(workspace).expect("remove temporary workspace");
    }

    #[cfg(unix)]
    #[test]
    fn checkout_path_rejects_existing_symlink_components() {
        let workspace = temporary_root("checkout-symlink");
        let root = workspace.join("lqxp");
        std::fs::create_dir(&root).expect("create project root");
        let outside = temporary_root("checkout-outside");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("create symlink");
        std::os::unix::fs::symlink(&outside, workspace.join("sibling-link"))
            .expect("create sibling symlink");

        assert!(web_config("link/web").checkout_path(&root).is_err());
        assert!(web_config("../sibling-link/web")
            .checkout_path(&root)
            .is_err());

        std::fs::remove_dir_all(workspace).expect("remove temporary workspace");
        std::fs::remove_dir_all(outside).expect("remove outside root");
    }
}
