use ene_plugin_proto::ToolError;

/// Errors from argument validation, configuration, or Home Assistant API handling.
///
/// Used by the pure functions behind each action; the HTTP layer reports
/// transport failures directly as [`ToolError`].
#[derive(Debug, thiserror::Error)]
pub enum HomeAssistantError {
    /// An argument failed validation.
    #[error("{0}")]
    InvalidArguments(String),
    /// The plugin has no usable configuration (missing base URL or token).
    #[error("{0}")]
    NotConfigured(String),
    /// Home Assistant rejected the request.
    #[error("{0}")]
    ApiFailure(String),
    /// Home Assistant returned a malformed response.
    #[error("{0}")]
    InvalidResponse(String),
    /// An invariant on the tool side broke.
    #[error("{0}")]
    Internal(String),
}

impl From<HomeAssistantError> for ToolError {
    fn from(error: HomeAssistantError) -> Self {
        match error {
            HomeAssistantError::InvalidArguments(message) => {
                ToolError::InvalidArguments { message }
            }
            HomeAssistantError::NotConfigured(message)
            | HomeAssistantError::ApiFailure(message)
            | HomeAssistantError::InvalidResponse(message) => ToolError::execution_failed(message),
            HomeAssistantError::Internal(message) => ToolError::internal(message),
        }
    }
}
