use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("configuration error: {0}")]
    ConfigError(String),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("provider error: {0}")]
    ProviderError(String),
    #[error("http error ({status_code:?}): {message}")]
    HttpError {
        message: String,
        status_code: Option<u16>,
        response_body: Option<String>,
    },
}
