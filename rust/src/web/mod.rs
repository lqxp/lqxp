mod command;
mod repository;

pub use command::{CommandRunner, SystemCommandRunner, WebError, WebFailureKind};
pub use repository::prepare;

#[cfg(test)]
pub use command::{sanitize_repo_url, CommandOutput, CommandRequest};
#[cfg(test)]
pub use repository::PreparedWeb;

#[cfg(test)]
mod tests;
