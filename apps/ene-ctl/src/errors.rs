use std::process::ExitCode;

use ene_client::ClientError;
use ene_config::typed::ConfigError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Transport, codec, and Host domain outcomes pass through from
    /// [`ene_client::ClientError`] unchanged.
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Client(ClientError::ServerOutcome(_)) => ExitCode::from(2),
            Self::Usage(_) | Self::Config(_) | Self::Client(_) => ExitCode::FAILURE,
        }
    }
}
