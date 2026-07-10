//! Shared surface acquire outcomes for wgpu presentation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcquireError {
    Reconfigure,
    Timeout,
    Fatal,
}
