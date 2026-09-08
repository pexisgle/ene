use thiserror::Error;

/// IPC transport and handshake failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(String),
    #[error("frame exceeds max_frame_bytes ({got} > {max})")]
    FrameTooLarge { got: usize, max: usize },
    #[error("core protocol ranges do not overlap")]
    CoreIncompatible,
    #[error("hello_ack named undeclared protocol {0}")]
    UndeclaredProtocol(String),
    #[error("manifest digest mismatch")]
    DigestMismatch,
    #[error("handshake rejected: {0}")]
    Rejected(String),
    #[error("unexpected message {0}")]
    Unexpected(String),
    #[error("tool {0} is not registered")]
    UnknownTool(String),
    #[error("call failed: {0}")]
    Call(String),
    #[error("peer closed")]
    Closed,
    #[error("bulk transfer is not supported on this platform")]
    BulkUnsupported,
    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}

impl IpcError {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "used as map_err(IpcError::codec)"
    )]
    pub(crate) fn codec(err: impl ToString) -> Self {
        Self::Codec(err.to_string())
    }
}
