//! CLI failure type shared by the binary and the programmatic client surface.
//!
//! Message rule: variants carry operations, payload-kind names, refs,
//! generations, and the Host's own operational reasons only — never secrets,
//! key material, or conversation bodies.

use std::process::ExitCode;

use ene_config::typed::ConfigError;

/// Usage text reported with every [`CliError::Usage`].
pub const USAGE: &str = "usage: ene-ctl [--config PATH] <command>\ncommands: setup [--show | --provider openai --model MODEL], status, send [--new | --round ROUND] TEXT..., watch --round ROUND, history [--limit N]";

/// CLI failure: bad arguments, configuration, transport, codec, or
/// Host-reported refusals and declines.
///
/// Message rule: variants carry operations, payload-kind names, refs,
/// generations, and the Host's own operational reasons only — never secrets,
/// key material, or conversation bodies. Codec messages rely on
/// `ene-plugin-ipc` diagnostics, which never echo frame bytes.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Command-line usage failure; the message always ends with [`USAGE`].
    #[error("{0}")]
    Usage(String),
    /// Layered configuration loading or validation failed.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Socket, stdio, or runtime movement failed; carries the operation and
    /// I/O kind only.
    #[error("transport: {0}")]
    Transport(String),
    /// Frame encode/decode failed; carries lengths and decoder reasons only.
    #[error("codec: {0}")]
    Codec(String),
    /// The Host terminally refused the request (incompatible version,
    /// unexpected payload kind, terminal management declines). Retrying the
    /// same request will not help. Exit code 1.
    #[error("server rejected: {0}")]
    ServerRejected(String),
    /// The Host answered with a retryable Ok-side domain outcome (stale
    /// round, held transition, stale base view, pending Owner confirmation,
    /// denied pairing, rejected auth proof). Exit code 2.
    #[error("server outcome: {0}")]
    ServerOutcome(String),
    /// The platform has no supported transport (non-Unix). Constructed by
    /// the Windows transport stubs and matched by [`CliError::exit_code`].
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}

impl CliError {
    /// Maps a failure to its process exit code: retryable server-side
    /// domain outcomes exit `2`, everything else exits `1`.
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
