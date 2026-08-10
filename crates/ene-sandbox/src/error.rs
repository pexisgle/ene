/// Sandbox initialization errors. All are fatal for the child: the plugin
/// never starts when a required layer cannot be applied.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The Linux kernel lacks the required feature.
    #[error("kernel does not support {0}")]
    Unsupported(&'static str),
    /// The requirement needs privileges the host does not have.
    #[error("{0} requires privileges the host lacks: {1}")]
    Privilege(&'static str, String),
    /// A syscall-based layer failed.
    #[error("{0} failed: {1}")]
    Syscall(&'static str, String),
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Windows API failure.
    #[error("Windows error {code} (0x{code:08X}): {message}")]
    Windows {
        /// Win32 error code.
        code: u32,
        /// Operation that failed.
        message: String,
    },
    /// The sandbox spec is internally inconsistent.
    #[error("invalid sandbox spec: {0}")]
    InvalidSpec(String),
    /// Landlock ruleset construction failed.
    #[error("landlock: {0}")]
    Landlock(String),
}
