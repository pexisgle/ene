use thiserror::Error;

/// Errors that can occur during configuration loading, schema generation, or
/// card resolution.
#[derive(Error, Debug)]
pub enum EneConfigError {
    /// An AI provider base URL is missing and no environment variable fallback
    /// was found.
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl {
        /// The environment variable that was checked.
        env_var: String,
    },
    /// An API key is missing and no environment variable fallback was found.
    #[error("Missing API key: set {env_var} or configure AI API Key")]
    MissingApiKey {
        /// The environment variable that was checked.
        env_var: String,
    },
    /// No character card has been loaded yet.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// I/O error while reading a character card file.
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    /// JSON deserialisation error.
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    /// Catch-all configuration error with a free-form message.
    #[error("Configuration error: {0}")]
    GenericConfigError(String),
    /// The on-disk config declares a schema version newer than this build
    /// understands. This typically means the file was written by a newer
    /// application version (a downgrade). The file is left untouched so a
    /// newer build can still read it.
    #[error(
        "config version {found} is newer than the supported version {supported}; \
         refusing to load to avoid corrupting data written by a newer build"
    )]
    ConfigVersionTooNew {
        /// The version number found in the on-disk file.
        found: u32,
        /// The highest version this build can read.
        supported: u32,
    },
    /// General I/O error.
    #[error("I/O error: {0}")]
    IoError(#[source] std::io::Error),
    /// A runtime prompt pack could not be read from the assets directory. The
    /// caller is expected to fall back to the compile-time embedded library.
    #[error("Failed to read prompt pack at {path}: {source}")]
    PromptPackRead {
        /// The pack path that was attempted.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Type alias for internal module usages.
pub type ConfigError = EneConfigError;
