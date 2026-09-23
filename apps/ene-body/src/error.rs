use crate::ipc::IpcError;

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("usage: {0}")]
    Usage(String),
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error("transport: {0}")]
    Transport(String),
    #[error("peer disconnected")]
    Disconnected,
    #[error("runtime: {0}")]
    Runtime(String),
}

impl BodyError {
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        1
    }
}
