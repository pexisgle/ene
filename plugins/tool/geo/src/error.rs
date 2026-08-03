use ene_plugin_proto::ToolError;

/// Errors from parsing, formatting, or validating geographic data.
///
/// Used by the pure functions behind each action; the HTTP layer reports
/// transport failures directly as [`ToolError`].
#[derive(Debug, thiserror::Error)]
pub enum GeoError {
    /// An argument failed validation.
    #[error("{0}")]
    InvalidArguments(String),
    /// The upstream API rejected the request.
    #[error("{0}")]
    ApiFailure(String),
    /// The upstream API returned a malformed response.
    #[error("{0}")]
    InvalidResponse(String),
    /// An invariant on the tool side broke (e.g. a constant URL no longer parses).
    #[error("{0}")]
    Internal(String),
}

impl From<GeoError> for ToolError {
    fn from(error: GeoError) -> Self {
        match error {
            GeoError::InvalidArguments(message) => ToolError::InvalidArguments { message },
            GeoError::ApiFailure(message) | GeoError::InvalidResponse(message) => {
                ToolError::execution_failed(message)
            }
            GeoError::Internal(message) => ToolError::internal(message),
        }
    }
}
