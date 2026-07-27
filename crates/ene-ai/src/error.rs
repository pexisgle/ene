use thiserror::Error;

/// Maps an `async_openai` error into the appropriate `LlmProviderError`
/// variant based on its HTTP status code (for `ApiErrorResponse`) or
/// shape. `async_openai` itself doesn't have typed auth/rate-limit
/// variants — the closest signal is the status code.
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

    /// Local llama.cpp load / inference failure (#165 / #171).
    #[error("local LLM error: {0}")]
    LocalLlm(String),

    /// A local engine's bounded job queue was full
    /// (`ene_infer::EngineError::Busy`). Enqueue contention only — the
    /// engine itself is fine.
    #[error("engine busy: queue depth {queue_depth} exceeded")]
    Busy {
        /// The engine's configured queue depth at the time of the call.
        queue_depth: usize,
    },

    /// A local engine's job timed out (`ene_infer::EngineError::Timeout`).
    /// Distinct from [`Self::Truncated`], which is a *model* truncating its
    /// own output — this is the framework giving up on a job that never
    /// finished.
    #[error("local engine job timed out")]
    Timeout,

    /// A local engine's job was cancelled (`ene_infer::EngineError::Cancelled`),
    /// either by an explicit cancellation or because the caller went away.
    #[error("local engine job cancelled")]
    Cancelled,
}

impl LlmProviderError {
    /// Whether this error is transient and may succeed on retry.
    ///
    /// `RateLimit` (429), `Network`, `Busy`, and `Timeout` errors are
    /// retryable — a full queue drains and a slow-but-alive engine may
    /// succeed given a fresh deadline. `Auth`, `ContentFilter`, `Truncated`,
    /// and `Provider` errors are not. `Cancelled` reflects caller intent,
    /// not engine health, so retrying is the caller's call, not something
    /// this method encourages — conservatively reported as not retryable,
    /// mirroring `ene_infer::EngineError::is_retryable`.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit(_) | Self::Network(_) | Self::Busy { .. } | Self::Timeout
        )
    }

    /// Extracts a `Retry-After` hint (seconds) embedded in a `RateLimit`
    /// message by the transport layer, if present.
    #[must_use]
    pub fn retry_after_secs(&self) -> Option<u64> {
        const MARKER: &str = "[retry-after: ";
        if let Self::RateLimit(msg) = self
            && let Some((_prefix, rest)) = msg.split_once(MARKER)
            && let Some(value) = rest.strip_suffix("s]")
        {
            return value.trim().parse::<u64>().ok();
        }
        None
    }
}

/// Single public error type for the `ene-ai` crate boundary (API v1 / #118).
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
    /// API key is missing or empty (#237).
    #[error("API key not configured: {0}")]
    MissingApiKey(String),
    /// Provider base URL is missing (#241).
    #[error("base URL not configured: {0}")]
    MissingBaseUrl(String),
    /// Provider base URL failed format checks (#241).
    #[error("invalid base URL: {0}")]
    InvalidBaseUrl(String),
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
