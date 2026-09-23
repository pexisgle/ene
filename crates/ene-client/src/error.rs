#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("codec: {0}")]
    Codec(String),
    #[error("server rejected: {0}")]
    ServerRejected(String),
    #[error("server outcome: {0}")]
    ServerOutcome(String),
    #[error(
        "the Host's public key changed: trusted {stored}, offered {offered}; verify the new pin yourself, then run `ene-ctl trust-host --pin {offered}`"
    )]
    HostPinMismatch { stored: String, offered: String },
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}
