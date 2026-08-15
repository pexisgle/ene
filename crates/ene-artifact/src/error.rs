pub type Result<T> = std::result::Result<T, ArtifactError>;

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("catalog signature verification failed: {0}")]
    BadSignature(String),
    #[error("catalog metadata expired at {expired_at_ms} (now {now_ms})")]
    ExpiredCatalog {
        /// Expiry as Unix milliseconds.
        expired_at_ms: u64,
        /// Current time as Unix milliseconds.
        now_ms: u64,
    },
    #[error("catalog rollback/digest-change rejected for '{artifact}': {detail}")]
    Rollback { artifact: String, detail: String },
    #[error("artifact '{0}' not found in catalog")]
    ArtifactNotFound(String),
    #[error("download exceeded maximum size of {max} bytes (got {got})")]
    SizeExceeded {
        /// Configured cap in bytes.
        max: u64,
        /// Observed size in bytes.
        got: u64,
    },
    #[error("digest mismatch for '{artifact}': expected {expected}, got {actual}")]
    DigestMismatch {
        artifact: String,
        /// Expected hex SHA-256.
        expected: String,
        /// Observed hex SHA-256.
        actual: String,
    },
    #[error("size mismatch for '{artifact}': expected {expected}, got {actual}")]
    SizeMismatch {
        artifact: String,
        /// Expected size in bytes.
        expected: u64,
        /// Observed size in bytes.
        actual: u64,
    },
    #[error("download failed for '{url}': {status}")]
    HttpStatus { url: String, status: u16 },
    #[error("download error for '{url}': {message}")]
    Transport { url: String, message: String },
    #[error("redirect rejected: {0}")]
    RedirectRejected(String),
    #[error("too many redirects (limit {limit})")]
    TooManyRedirects { limit: usize },
    #[error("unsupported URL scheme '{scheme}' (https required)")]
    UnsupportedScheme { scheme: String },
    #[error("CAS object '{0}' not found")]
    CasMissing(String),
    /// The CAS object path escaped the store root.
    #[error("invalid CAS digest '{0}'")]
    InvalidDigest(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Ed25519 key parsing failure.
    #[error("key error: {0}")]
    Key(String),
    /// The operation was cancelled (download interrupted by the user).
    #[error("operation cancelled")]
    Cancelled,
    /// An archive payload violated the extraction safety rules.
    #[error("unsafe archive payload: {0}")]
    UnsafeArchive(String),
}

impl ArtifactError {
    #[must_use]
    pub fn transport(url: &str, source: &reqwest::Error) -> Self {
        Self::Transport {
            url: url.to_string(),
            message: source.to_string(),
        }
    }
}
