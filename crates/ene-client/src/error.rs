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

#[derive(Debug, thiserror::Error)]
pub enum EnqueueFailure {
    #[error("frame encoding failed: {0}")]
    Encode(#[source] ene_api::codec::CodecError),
    #[error("the outbound request queue did not become writable in time")]
    QueueWaitTimedOut,
    #[error("the connection writer is closed")]
    WriterClosed,
}

#[derive(Debug, thiserror::Error)]
pub enum ResponseWaitFailure {
    #[error("the Host response timed out")]
    TimedOut,
    #[error(transparent)]
    Client(#[from] ClientError),
}

#[derive(Debug, thiserror::Error)]
pub enum RequestFailure {
    #[error("the request was not sent: {0}")]
    NotSent(#[source] EnqueueFailure),
    #[error("the request outcome is unknown: {0}")]
    OutcomeUnknown(#[source] ResponseWaitFailure),
}
