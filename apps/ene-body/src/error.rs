//! Process-level errors. Display strings never echo IPC body bytes.

use crate::ipc::IpcError;

/// Failures that end the process or reject CLI usage.
///
/// Domain outcomes (`GpuFail`, `AssetFail`) travel on the projection IPC
/// and are not represented here: those are not technical process failures.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("usage: {0}")]
    Usage(String),
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error("transport: {0}")]
    Transport(String),
    #[error("runtime: {0}")]
    Runtime(String),
}

impl BodyError {
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        1
    }
}
