use std::process::ExitCode;

use ene_config::typed::ConfigError;

pub const USAGE: &str = "usage: ene-ctl [--config PATH] <command>\ncommands: setup [--show | --provider openai --model MODEL], status, send [--new | --round ROUND] TEXT..., watch --round ROUND, history [--limit N]";

/// Message rule: variants carry operations, payload-kind names, refs,
/// generations, and the Host's own operational reasons only — never secrets,
/// key material, or conversation bodies. Codec messages rely on
/// `ene-plugin-ipc` diagnostics, which never echo frame bytes.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The message always ends with [`USAGE`].
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Carries the operation and I/O kind only.
    #[error("transport: {0}")]
    Transport(String),
    /// Carries lengths and decoder reasons only.
    #[error("codec: {0}")]
    Codec(String),
    /// Terminal Host refusal (incompatible version, unexpected payload kind,
    /// terminal management declines); retrying the same request will not help.
    #[error("server rejected: {0}")]
    ServerRejected(String),
    /// Retryable Ok-side domain outcome from the Host.
    #[error("server outcome: {0}")]
    ServerOutcome(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}

impl CliError {
    /// Retryable server-side domain outcomes exit `2`; everything else exits
    /// `1`.
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
