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
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}
