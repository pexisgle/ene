/// Client-library failures. Variants carry operations, payload-kind names,
/// refs, and Host operational reasons only — never secrets or bodies.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Carries the operation and I/O kind only.
    #[error("transport: {0}")]
    Transport(String),
    /// Carries lengths and decoder reasons only.
    #[error("codec: {0}")]
    Codec(String),
    /// Terminal Host refusal; retrying the same request will not help.
    #[error("server rejected: {0}")]
    ServerRejected(String),
    /// Retryable Ok-side domain outcome from the Host.
    #[error("server outcome: {0}")]
    ServerOutcome(String),
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(&'static str),
}
