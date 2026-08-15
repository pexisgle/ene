/// Result alias for the artifact crate.
pub type Result<T> = std::result::Result<T, ArtifactError>;

/// Errors produced by the artifact pipeline.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// The catalog signature did not verify.
    #[error("catalog signature verification failed: {0}")]
    BadSignature(String),
    /// The catalog metadata has expired.
    #[error("catalog metadata expired at {expired_at_ms} (now {now_ms})")]
    ExpiredCatalog {
        /// Expiry as Unix milliseconds.
        expired_at_ms: u64,
        /// Current time as Unix milliseconds.
        now_ms: u64,
    },
    /// The catalog would downgrade or change an installed artifact.
    #[error("catalog rollback/digest-change rejected for '{artifact}': {detail}")]
    Rollback {
        /// Artifact id that was rejected.
        artifact: String,
        /// Human-readable explanation.
        detail: String,
    },
    /// The requested artifact is not in the catalog.
    #[error("artifact '{0}' not found in catalog")]
    ArtifactNotFound(String),
    /// The download exceeded the configured maximum size.
    #[error("download exceeded maximum size of {max} bytes (got {got})")]
    SizeExceeded {
        /// Configured cap in bytes.
        max: u64,
        /// Observed size in bytes.
        got: u64,
    },
    /// The downloaded bytes did not match the catalog digest.
    #[error("digest mismatch for '{artifact}': expected {expected}, got {actual}")]
    DigestMismatch {
        /// Artifact being installed.
        artifact: String,
        /// Expected hex SHA-256.
        expected: String,
        /// Observed hex SHA-256.
        actual: String,
    },
    /// The downloaded bytes did not match the catalog size.
    #[error("size mismatch for '{artifact}': expected {expected}, got {actual}")]
    SizeMismatch {
        /// Artifact being installed.
        artifact: String,
        /// Expected size in bytes.
        expected: u64,
        /// Observed size in bytes.
        actual: u64,
    },
    /// HTTP-level failure while downloading.
    #[error("download failed for '{url}': {status}")]
    HttpStatus {
        /// Requested URL.
        url: String,
        /// HTTP status code.
        status: u16,
    },
    /// Network/transport failure while downloading.
    #[error("download error for '{url}': {message}")]
    Transport {
        /// Requested URL.
        url: String,
        /// Underlying failure description.
        message: String,
    },
    /// A redirect was rejected by the caller's policy.
    #[error("redirect rejected: {0}")]
    RedirectRejected(String),
    /// Too many redirects.
    #[error("too many redirects (limit {limit})")]
    TooManyRedirects {
        /// Redirect hop limit.
        limit: usize,
    },
    /// The URL scheme is not https.
    #[error("unsupported URL scheme '{scheme}' (https required)")]
    UnsupportedScheme {
        /// The offending scheme.
        scheme: String,
    },
    /// A CAS object is missing.
    #[error("CAS object '{0}' not found")]
    CasMissing(String),
    /// The CAS object path escaped the store root.
    #[error("invalid CAS digest '{0}'")]
    InvalidDigest(String),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization failure.
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
    /// Wraps a reqwest error with the URL it came from.
    #[must_use]
    pub fn transport(url: &str, source: &reqwest::Error) -> Self {
        Self::Transport {
            url: url.to_string(),
            message: source.to_string(),
        }
    }
}
