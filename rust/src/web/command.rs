use std::{fmt, io, path::PathBuf, process::Stdio};

use async_trait::async_trait;
use regex::Regex;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl CommandRequest {
    pub fn new(program: impl Into<String>, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            program: program.into(),
            args,
            cwd,
        }
    }

    pub fn sanitized_description(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(sanitize_repo_url(&self.program));
        parts.extend(self.args.iter().map(|arg| sanitize_repo_url(arg)));
        parts.join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, request: &CommandRequest) -> io::Result<CommandOutput>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCommandRunner;

#[async_trait]
impl CommandRunner for SystemCommandRunner {
    async fn run(&self, request: &CommandRequest) -> io::Result<CommandOutput> {
        let output = Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await?;

        Ok(CommandOutput {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

pub fn sanitize_repo_url(raw: &str) -> String {
    Regex::new(r"https://[^/@\s]+@")
        .expect("credential redaction regex is valid")
        .replace_all(raw, "https://***@")
        .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebFailureKind {
    LocalCheckout,
    External,
    System,
    Configuration,
    Source,
    Build,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebError {
    kind: WebFailureKind,
    stage: String,
    command: Option<String>,
    status: Option<i32>,
    detail: String,
}

impl WebError {
    pub fn new(
        kind: WebFailureKind,
        stage: impl Into<String>,
        request: Option<&CommandRequest>,
        status: Option<i32>,
        detail: impl AsRef<str>,
    ) -> Self {
        Self {
            kind,
            stage: stage.into(),
            command: request.map(CommandRequest::sanitized_description),
            status,
            detail: sanitize_repo_url(detail.as_ref()),
        }
    }

    pub fn kind(&self) -> WebFailureKind {
        self.kind
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "web preparation failed during {}", self.stage)?;
        if let Some(command) = &self.command {
            write!(formatter, " ({command})")?;
        }
        if let Some(status) = self.status {
            write!(formatter, " [exit {status}]")?;
        }
        if !self.detail.trim().is_empty() {
            write!(formatter, ": {}", self.detail.trim())?;
        }
        Ok(())
    }
}

impl std::error::Error for WebError {}
