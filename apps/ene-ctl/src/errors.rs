use std::process::ExitCode;

use ene_client::ClientError;
use ene_config::typed::ConfigError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("transport: {0}")]
    Transport(String),
    #[error("codec: {0}")]
    Codec(String),
    #[error("server rejected: {0}")]
    ServerRejected(String),
    #[error("server outcome: {0}")]
    ServerOutcome(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}

impl From<ClientError> for CliError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Transport(message) => Self::Transport(message),
            ClientError::Codec(message) => Self::Codec(message),
            ClientError::ServerRejected(message) => Self::ServerRejected(message),
            ClientError::ServerOutcome(message) => Self::ServerOutcome(message),
            ClientError::UnsupportedPlatform(message) => Self::UnsupportedPlatform(message),
        }
    }
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::ServerOutcome(_) => ExitCode::from(2),
            Self::Usage(_)
            | Self::Config(_)
            | Self::Transport(_)
            | Self::Codec(_)
            | Self::ServerRejected(_)
            | Self::UnsupportedPlatform(_) => ExitCode::FAILURE,
        }
    }
}
