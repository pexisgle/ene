use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl { env_var: String },
    #[error("Missing API key: set {env_var} or configure AI API Key")]
    MissingApiKey { env_var: String },
    #[error("Character card not loaded")]
    NoCharacterCard,
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Configuration error: {0}")]
    GenericConfigError(String),
}
