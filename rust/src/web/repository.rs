use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::core::config::{Config, WebRevision};

use super::{
    command::{CommandOutput, CommandRequest},
    CommandRunner, WebError, WebFailureKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    ToolGit,
    ToolBun,
    Clone,
    InspectCheckout,
    SetOrigin,
    Fetch,
    ValidateTag,
    ResolveRevision,
    Checkout,
    Reset,
    Clean,
    DependencyCleanup,
    BunInstall,
    BunBuild,
    VerifyOutput,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::ToolGit => "git tool validation",
            Self::ToolBun => "Bun tool validation",
            Self::Clone => "repository clone",
            Self::InspectCheckout => "checkout inspection",
            Self::SetOrigin => "origin configuration",
            Self::Fetch => "remote fetch",
            Self::ValidateTag => "tag validation",
            Self::ResolveRevision => "revision resolution",
            Self::Checkout => "revision checkout",
            Self::Reset => "checkout reset",
            Self::Clean => "checkout clean",
            Self::DependencyCleanup => "dependency cleanup",
            Self::BunInstall => "Bun install",
            Self::BunBuild => "Bun build",
            Self::VerifyOutput => "built index verification",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWeb {
    pub checkout: PathBuf,
    pub revision: String,
    pub index: PathBuf,
}

pub async fn prepare(
    config: &Config,
    root: &Path,
    runner: &dyn CommandRunner,
) -> Result<PreparedWeb, WebError> {
    let revision_mode = config.web.revision().map_err(|err| {
        WebError::new(
            WebFailureKind::Configuration,
            "web configuration",
            None,
            None,
            err.to_string(),
        )
    })?;
    let checkout = config.web.checkout_path(root).map_err(|err| {
        WebError::new(
            WebFailureKind::Configuration,
            "checkout path validation",
            None,
            None,
            err.to_string(),
        )
    })?;

    run(
        runner,
        CommandRequest::new("git", vec!["--version".into()], root.to_path_buf()),
        Stage::ToolGit,
        WebFailureKind::System,
    )
    .await?;
    run(
        runner,
        CommandRequest::new("bun", vec!["--version".into()], root.to_path_buf()),
        Stage::ToolBun,
        WebFailureKind::System,
    )
    .await?;

    let revision = match prepare_once(config, root, &checkout, &revision_mode, runner).await {
        Ok(revision) => revision,
        Err(error) if error.kind() == WebFailureKind::LocalCheckout => {
            let checked_again = config.web.checkout_path(root).map_err(|err| {
                WebError::new(
                    WebFailureKind::Configuration,
                    "checkout recovery path validation",
                    None,
                    None,
                    err.to_string(),
                )
            })?;
            if checked_again != checkout {
                return Err(WebError::new(
                    WebFailureKind::Configuration,
                    "checkout recovery path validation",
                    None,
                    None,
                    "checkout path changed during recovery",
                ));
            }
            warn!(path = %checkout.display(), cause = %error, "recreating corrupt web checkout");
            remove_checkout(&checkout).await?;
            prepare_once(config, root, &checkout, &revision_mode, runner).await?
        }
        Err(error) => return Err(error),
    };
    let index = PathBuf::from(&config.network.public_dir).join(&config.network.webchat_index);
    if !index.is_file() {
        return Err(WebError::new(
            WebFailureKind::Output,
            Stage::VerifyOutput.label(),
            None,
            None,
            format!(
                "built web index is missing or not a file: {}",
                index.display()
            ),
        ));
    }

    info!(revision, index = %index.display(), "web client prepared");
    Ok(PreparedWeb {
        checkout,
        revision,
        index,
    })
}

async fn remove_checkout(checkout: &Path) -> Result<(), WebError> {
    let metadata = match tokio::fs::symlink_metadata(checkout).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(WebError::new(
                WebFailureKind::System,
                "checkout recovery inspection",
                None,
                None,
                err.to_string(),
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(WebError::new(
            WebFailureKind::Configuration,
            "checkout recovery inspection",
            None,
            None,
            "checkout path became a symlink during recovery",
        ));
    }
    let result = if metadata.is_dir() {
        tokio::fs::remove_dir_all(checkout).await
    } else {
        tokio::fs::remove_file(checkout).await
    };
    result.map_err(|err| {
        WebError::new(
            WebFailureKind::System,
            "checkout recovery removal",
            None,
            None,
            err.to_string(),
        )
    })
}

async fn prepare_once(
    config: &Config,
    root: &Path,
    checkout: &Path,
    revision_mode: &WebRevision,
    runner: &dyn CommandRunner,
) -> Result<String, WebError> {
    if checkout.exists() {
        git(
            runner,
            root,
            checkout,
            &["rev-parse", "--is-inside-work-tree"],
            Stage::InspectCheckout,
            WebFailureKind::LocalCheckout,
        )
        .await?;
    } else {
        if let Some(parent) = checkout.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                WebError::new(
                    WebFailureKind::System,
                    Stage::Clone.label(),
                    None,
                    None,
                    err.to_string(),
                )
            })?;
        }
        run(
            runner,
            CommandRequest::new(
                "git",
                vec![
                    "clone".into(),
                    "--no-checkout".into(),
                    config.web.repo.clone(),
                    checkout.to_string_lossy().into_owned(),
                ],
                root.to_path_buf(),
            ),
            Stage::Clone,
            WebFailureKind::External,
        )
        .await?;
    }

    git(
        runner,
        root,
        checkout,
        &["remote", "set-url", "origin", &config.web.repo],
        Stage::SetOrigin,
        WebFailureKind::LocalCheckout,
    )
    .await?;
    git(
        runner,
        root,
        checkout,
        &["fetch", "--force", "--prune", "--tags", "origin"],
        Stage::Fetch,
        WebFailureKind::External,
    )
    .await?;

    let revision_output = match revision_mode {
        WebRevision::Fresh => {
            git(
                runner,
                root,
                checkout,
                &["remote", "set-head", "origin", "--auto"],
                Stage::ResolveRevision,
                WebFailureKind::Source,
            )
            .await?;
            git(
                runner,
                root,
                checkout,
                &["rev-parse", "--verify", "refs/remotes/origin/HEAD^{commit}"],
                Stage::ResolveRevision,
                WebFailureKind::Source,
            )
            .await?
        }
        WebRevision::Tag(tag) => {
            let tag_ref = format!("refs/tags/{tag}");
            git(
                runner,
                root,
                checkout,
                &["check-ref-format", &tag_ref],
                Stage::ValidateTag,
                WebFailureKind::Configuration,
            )
            .await?;
            let commit_ref = format!("{tag_ref}^{{commit}}");
            git(
                runner,
                root,
                checkout,
                &["rev-parse", "--verify", &commit_ref],
                Stage::ResolveRevision,
                WebFailureKind::Source,
            )
            .await?
        }
    };
    let revision = revision_output.stdout.trim().to_owned();
    if revision.is_empty() {
        return Err(WebError::new(
            WebFailureKind::Source,
            Stage::ResolveRevision.label(),
            None,
            revision_output.status,
            "Git returned an empty revision",
        ));
    }

    git(
        runner,
        root,
        checkout,
        &["checkout", "--detach", "--force", &revision],
        Stage::Checkout,
        WebFailureKind::LocalCheckout,
    )
    .await?;
    git(
        runner,
        root,
        checkout,
        &["reset", "--hard", &revision],
        Stage::Reset,
        WebFailureKind::LocalCheckout,
    )
    .await?;
    git(
        runner,
        root,
        checkout,
        &["clean", "-fdx"],
        Stage::Clean,
        WebFailureKind::LocalCheckout,
    )
    .await?;
    remove_dependency_tree(&checkout.join("node_modules")).await?;
    run(
        runner,
        CommandRequest::new("bun", vec!["install".into()], checkout.to_path_buf()),
        Stage::BunInstall,
        WebFailureKind::Source,
    )
    .await?;
    run(
        runner,
        CommandRequest::new(
            "bun",
            vec!["run".into(), "build".into()],
            checkout.to_path_buf(),
        ),
        Stage::BunBuild,
        WebFailureKind::Build,
    )
    .await?;
    Ok(revision)
}

async fn remove_dependency_tree(path: &Path) -> Result<(), WebError> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(WebError::new(
                WebFailureKind::System,
                Stage::DependencyCleanup.label(),
                None,
                None,
                err.to_string(),
            ));
        }
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    };
    result.map_err(|err| {
        WebError::new(
            WebFailureKind::System,
            Stage::DependencyCleanup.label(),
            None,
            None,
            err.to_string(),
        )
    })
}

async fn git(
    runner: &dyn CommandRunner,
    root: &Path,
    checkout: &Path,
    args: &[&str],
    stage: Stage,
    failure_kind: WebFailureKind,
) -> Result<CommandOutput, WebError> {
    let mut full_args = vec!["-C".to_owned(), checkout.to_string_lossy().into_owned()];
    full_args.extend(args.iter().map(|arg| (*arg).to_owned()));
    run(
        runner,
        CommandRequest::new("git", full_args, root.to_path_buf()),
        stage,
        failure_kind,
    )
    .await
}

async fn run(
    runner: &dyn CommandRunner,
    request: CommandRequest,
    stage: Stage,
    failure_kind: WebFailureKind,
) -> Result<CommandOutput, WebError> {
    info!(stage = stage.label(), "preparing web client");
    let output = runner.run(&request).await.map_err(|err| {
        WebError::new(
            WebFailureKind::System,
            stage.label(),
            Some(&request),
            None,
            err.to_string(),
        )
    })?;
    if output.success() {
        return Ok(output);
    }

    let detail = output.stderr.trim();
    let lowercase = detail.to_ascii_lowercase();
    let kind = if [
        "permission denied",
        "read-only file system",
        "no space left on device",
    ]
    .iter()
    .any(|needle| lowercase.contains(needle))
    {
        WebFailureKind::System
    } else if stage == Stage::Fetch && is_stale_git_lock(&lowercase) {
        WebFailureKind::LocalCheckout
    } else {
        failure_kind
    };
    Err(WebError::new(
        kind,
        stage.label(),
        Some(&request),
        output.status,
        detail,
    ))
}

fn is_stale_git_lock(detail: &str) -> bool {
    detail.contains("another git process seems to be running")
        || detail.contains("index.lock")
        || detail.contains("shallow.lock")
        || detail.contains("cannot lock ref")
        || (detail.contains("unable to create")
            && detail.contains(".lock")
            && detail.contains("file exists"))
}
