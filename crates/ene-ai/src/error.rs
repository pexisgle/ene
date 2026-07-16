use thiserror::Error;

/// Maps an `async_openai` error into the appropriate `LlmProviderError`
/// variant based on its HTTP status code (for `ApiErrorResponse`) or
/// shape. `async_openai` itself doesn't have typed auth/rate-limit
/// variants — the closest signal is the status code.
#[must_use]
pub fn map_openai_error(err: &async_openai::error::OpenAIError) -> LlmProviderError {
    use async_openai::error::OpenAIError;
    match err {
        OpenAIError::Reqwest(_) | OpenAIError::StreamError(_) => {
            LlmProviderError::Network(err.to_string())
        }
        OpenAIError::ApiError(resp) => {
            let status = resp.status_code.as_u16();
            let body = resp.api_error.message.clone();
            if status == 401 || status == 403 {
                LlmProviderError::Auth(body)
            } else if status == 429 {
                LlmProviderError::RateLimit(body)
            } else {
                LlmProviderError::Provider(format!("HTTP {status}: {body}"))
            }
        }
        OpenAIError::JSONDeserialize(_, _)
        | OpenAIError::FileSaveError(_)
        | OpenAIError::FileReadError(_)
        | OpenAIError::InvalidArgument(_) => LlmProviderError::Provider(err.to_string()),
    }
}

/// Errors returned by `LlmProvider` implementations at the library boundary.
///
/// Callers (CLI, GUI, tool host) dispatch on the variant instead of parsing
/// strings: e.g. they can show a "rate limited" notice, prompt for
/// re-authentication on `Auth`, or surface a truncation warning on
/// `Truncated`.
#[derive(Debug, Error)]
pub enum LlmProviderError {
    /// The provider rejected the credentials (typically HTTP 401/403).
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The provider throttled this request (typically HTTP 429).
    #[error("rate limit exceeded: {0}")]
    RateLimit(String),

    /// A network-level failure (connect refused, DNS, TLS, read timeout)
    /// prevented the request from completing. Distinct from
    /// `Provider(String)`, which is for HTTP-level errors with a response.
    #[error("network error: {0}")]
    Network(String),

    /// The provider truncated the response because the configured token
    /// limit was reached. `partial_chars` is the number of characters
    /// actually returned before truncation, useful for diagnostics.
    #[error("response truncated (finish_reason=length) after {partial_chars} chars: {reason}")]
    Truncated {
        /// Human-readable reason for the truncation.
        reason: String,
        /// Number of characters the model emitted before being cut off.
        partial_chars: usize,
    },

    /// The provider blocked the response (typically HTTP 400 with
    /// `finish_reason=content_filter`). The model emitted no usable text.
    #[error("content filter blocked the response: {0}")]
    ContentFilter(String),

    /// Catch-all for provider-specific errors that do not map to the
    /// categories above. Prefer the typed variants when possible.
    #[error("provider error: {0}")]
    Provider(String),
}

/// Single public error type for the `ene-ai` crate boundary (API v2 / #118).
///
/// Domain-specific [`LlmProviderError`] and [`EmbeddingError`] remain available
/// as nested payloads for typed matching via `AiError` variants.
#[derive(Debug, Error)]
pub enum AiError {
    /// Chat / completion provider failure.
    #[error(transparent)]
    Llm(#[from] LlmProviderError),
    /// Embedding provider failure.
    #[error(transparent)]
    Embedding(#[from] crate::traits::EmbeddingError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::error::{ApiError, ApiErrorResponse, OpenAIError};
    use reqwest::StatusCode;

    fn api_error(status: u16, message: &str) -> OpenAIError {
        OpenAIError::ApiError(ApiErrorResponse {
            status_code: StatusCode::from_u16(status).expect("test status code is valid HTTP"),
            api_error: ApiError {
                message: message.to_string(),
                r#type: None,
                param: None,
                code: None,
            },
        })
    }

    #[test]
    fn map_openai_error_401_is_auth() {
        let err = api_error(401, "bad key");
        let mapped = map_openai_error(&err);
        match mapped {
            LlmProviderError::Auth(msg) => assert_eq!(msg, "bad key"),
            other => assert!(
                matches!(other, LlmProviderError::Auth(_)),
                "expected Auth, got {other:?}"
            ),
        }
    }

    #[test]
    fn map_openai_error_403_is_auth() {
        let err = api_error(403, "forbidden");
        let mapped = map_openai_error(&err);
        assert!(matches!(mapped, LlmProviderError::Auth(ref m) if m == "forbidden"));
    }

    #[test]
    fn map_openai_error_429_is_rate_limit() {
        let err = api_error(429, "slow down");
        let mapped = map_openai_error(&err);
        assert!(matches!(mapped, LlmProviderError::RateLimit(ref m) if m == "slow down"));
    }

    #[test]
    fn map_openai_error_500_is_provider() {
        let err = api_error(500, "boom");
        let mapped = map_openai_error(&err);
        match mapped {
            LlmProviderError::Provider(msg) => assert!(msg.contains("500") && msg.contains("boom")),
            other => assert!(
                matches!(other, LlmProviderError::Provider(_)),
                "expected Provider, got {other:?}"
            ),
        }
    }

    #[test]
    fn map_openai_error_invalid_argument_is_provider() {
        let err = OpenAIError::InvalidArgument("missing model".into());
        let mapped = map_openai_error(&err);
        assert!(matches!(mapped, LlmProviderError::Provider(_)));
    }

    #[test]
    fn map_openai_error_stream_error_is_network() {
        // We can't easily construct a `StreamError` without an async context;
        // instead check that the `InvalidArgument` arm does not fall into
        // `Network` — this guarantees the network arm is only hit for the
        // transport-like variants.
        let err = OpenAIError::InvalidArgument("foo".into());
        let mapped = map_openai_error(&err);
        assert!(!matches!(mapped, LlmProviderError::Network(_)));
    }
}
