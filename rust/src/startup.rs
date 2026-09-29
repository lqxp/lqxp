use std::path::Path;

use crate::{
    core::config::Config,
    web::{prepare, CommandRunner, WebError},
};

/// In dev (`cargo run` without `PRODUCTION=1`), never deploy the remote
/// client: serve the local files from `publicDir`.
/// - `LQXP_SKIP_WEB_PREPARE=1` forces the skip even in prod.
/// - `LQXP_FORCE_WEB_PREPARE=1` forces the deploy even in dev.
pub fn should_skip_web_prepare() -> bool {
    should_skip_web_prepare_with(
        std::env::var("PRODUCTION").is_ok(),
        std::env::var("LQXP_SKIP_WEB_PREPARE").is_ok(),
        std::env::var("LQXP_FORCE_WEB_PREPARE").is_ok(),
    )
}

fn should_skip_web_prepare_with(prod: bool, skip: bool, force: bool) -> bool {
    if skip {
        return true;
    }
    if !prod && !force {
        return true;
    }
    false
}

pub async fn startup_preflight(
    config: Config,
    root: &Path,
    runner: &dyn CommandRunner,
) -> Result<Config, WebError> {
    if should_skip_web_prepare() {
        tracing::info!(
            "dev mode: skipping web client deploy, using local files from {}",
            config.network.public_dir
        );
        return Ok(config);
    }
    prepare(&config, root, runner).await?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::{io, path::PathBuf};

    use async_trait::async_trait;

    use crate::{
        core::config::Config,
        web::{CommandOutput, CommandRequest, CommandRunner, WebFailureKind},
    };

    use super::{should_skip_web_prepare_with, startup_preflight};

    struct MissingBunRunner;

    #[async_trait]
    impl CommandRunner for MissingBunRunner {
        async fn run(&self, request: &CommandRequest) -> io::Result<CommandOutput> {
            if request.program == "bun" {
                return Err(io::Error::new(io::ErrorKind::NotFound, "bun is missing"));
            }
            Ok(CommandOutput {
                status: Some(0),
                stdout: "git version 2.51.0\n".to_owned(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn startup_preflight_stops_before_server_side_effects() {
        // `startup_preflight` reads the global env: force the `prepare` path
        // even when `PRODUCTION` is absent (`cargo test` / `cargo run` case).
        // No other test may touch these vars in parallel.
        std::env::set_var("LQXP_FORCE_WEB_PREPARE", "1");
        let root =
            std::env::temp_dir().join(format!("lqxp-startup-preflight-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temporary root");
        let upload_dir = root.join("files/uploads");
        let database = root.join("files/qxp.sqlite");
        let mut config = Config::default();
        config.web.directory = "web".to_owned();
        config.network.public_dir = root.join("web/dist").to_string_lossy().into_owned();
        config.network.upload_dir = upload_dir.to_string_lossy().into_owned();
        config.database.url = format!("sqlite://{}", database.display());

        let error = startup_preflight(config, &root, &MissingBunRunner)
            .await
            .expect_err("missing Bun must stop startup");

        assert_eq!(error.kind(), WebFailureKind::System);
        assert!(!PathBuf::from(&database).exists());
        assert!(!upload_dir.exists());
        std::fs::remove_dir_all(root).expect("remove temporary root");
        std::env::remove_var("LQXP_FORCE_WEB_PREPARE");
    }

    #[test]
    fn dev_skips_web_deploy_without_force() {
        // `cargo run` without PRODUCTION -> skip, local files.
        assert!(should_skip_web_prepare_with(false, false, false));
        // Opt-in to force the deploy in dev.
        assert!(!should_skip_web_prepare_with(false, false, true));
        // Prod -> deploy unless explicitly opted out.
        assert!(!should_skip_web_prepare_with(true, false, false));
        assert!(should_skip_web_prepare_with(true, true, false));
        assert!(should_skip_web_prepare_with(false, true, false));
    }
}
