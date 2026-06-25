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
    /// General I/O error.
    #[error("I/O error: {0}")]
    IoError(#[source] std::io::Error),
}

/// Type alias for internal module usages.
pub type ConfigError = EneConfigError;
