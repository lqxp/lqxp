use std::path::Path;

use crate::{
    core::config::Config,
    web::{prepare, CommandRunner, WebError},
};

pub async fn startup_preflight(
    config: Config,
    root: &Path,
    runner: &dyn CommandRunner,
) -> Result<Config, WebError> {
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

    use super::startup_preflight;

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
    }
}
