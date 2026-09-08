use thiserror::Error;

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("asset not found: {0}")]
    NotFound(String),
    #[error("version not found: {asset} {version}")]
    VersionNotFound { asset: String, version: String },
    #[error("url is not on the catalog allowlist")]
    UrlNotAllowed,
    #[error("sha256 mismatch")]
    DigestMismatch,
    #[error("platform not supported")]
    UnsupportedPlatform,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("download: {0}")]
    Download(String),
    #[error("archive: {0}")]
    Archive(String),
    #[error("job not found: {0}")]
    JobNotFound(String),
}
