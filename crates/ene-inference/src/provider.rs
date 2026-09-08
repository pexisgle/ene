//! `OpenAI` Responses API transport for inference dispatch.
//!
//! [`OpenAiResponsesTransport`] posts one `{"model", "input", "stream": false}`
//! body per [`ProviderTransport::complete`] call. Key material never rests on
//! the transport: each call borrows the bearer inside
//! [`CredentialStore::with_bearer`] and only the owned [`reqwest::Request`]
//! escapes the closure. Error strings carry status classes only, never URLs,
//! keys, or bodies. There is no retry, no streaming, and no model fallback.
//!
//! [`ProviderTransport::complete`] itself performs HTTPS I/O and is therefore
//! covered by integration tests through [`crate::fake::FakeProviderTransport`],
//! never by unit tests here; the pure `parse_response()` mapping below carries
//! the unit tests.

use std::time::Duration;

use ene_credential::{CredentialRef, CredentialStore, CredentialTechnicalError};
use serde::Deserialize;

use super::{
    InferenceTechnicalError, MAX_INPUT_CHARS, ProviderRequest, ProviderResponse, ProviderTransport,
    RawUsage,
};

/// Base URL for the `OpenAI` API; tests inject a local URL instead.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Time allowed to establish the provider connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Time allowed for a whole provider call, connect through body.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// HTTPS transport for the `OpenAI` Responses API (`POST /v1/responses`).
///
/// Holds the base URL, the built [`reqwest::Client`], the non-secret
/// [`CredentialRef`], and the bearer store. No field ever holds key material:
/// the bearer is borrowed transiently inside [`CredentialStore::with_bearer`]
/// on each call.
///
/// The store is a generic `S: CredentialStore` rather than a trait object
/// because [`CredentialStore::with_bearer`] is generic over its closure return
/// type, which makes the trait not dyn-compatible.
pub struct OpenAiResponsesTransport<S> {
    base_url: String,
    http: reqwest::Client,
    credential: CredentialRef,
    store: S,
}

impl<S> core::fmt::Debug for OpenAiResponsesTransport<S> {
    /// Renders the base URL and credential ref; the HTTP client and the store
    /// render opaque so no bearer material can leak through logging.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenAiResponsesTransport")
            .field("base_url", &self.base_url)
            .field("http", &"<http-client>")
            .field("credential", &self.credential)
            .field("store", &"<credential-store>")
            .finish()
    }
}

impl<S: CredentialStore> OpenAiResponsesTransport<S> {
    /// Creates a transport against `base_url` billed to `credential`.
    ///
    /// The client enforces [`CONNECT_TIMEOUT`] and [`REQUEST_TIMEOUT`]. No
    /// I/O happens here; pass [`DEFAULT_BASE_URL`] for production.
    pub fn new(base_url: impl Into<String>, credential: CredentialRef, store: S) -> Self {
        let http = match reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };
        Self {
            base_url: base_url.into(),
            http,
            credential,
            store,
        }
    }
}

impl<S: CredentialStore> ProviderTransport for OpenAiResponsesTransport<S> {
    /// Runs one Responses API completion with no retries or side effects
    /// beyond the call.
    ///
    /// Over-limit input is rejected defensively here, but core guarantees the
    /// pre-check: such input normally surfaces as `NotSent(OverLimit)` from
    /// [`crate::send`] without ever reaching the transport.
    async fn complete(
        &self,
        req: ProviderRequest,
    ) -> Result<ProviderResponse, InferenceTechnicalError> {
        if req.input.chars().count() > MAX_INPUT_CHARS {
            return Err(InferenceTechnicalError::ProviderTransportFailed(format!(
                "input over limit: {MAX_INPUT_CHARS} chars"
            )));
        }
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/responses");
        let body = serde_json::json!({
            "model": req.model,
            "input": req.input,
            "stream": false,
        });
        let build = self
            .store
            .with_bearer(&self.credential, |key| {
                self.http
                    .post(url.as_str())
                    .bearer_auth(key)
                    .json(&body)
                    .build()
            })
            .map_err(|CredentialTechnicalError::StorageUnavailable { reason }| {
                InferenceTechnicalError::ProviderTransportFailed(format!(
                    "credential unavailable: {reason}"
                ))
            })?;
        let request = build.map_err(|_| {
            InferenceTechnicalError::ProviderTransportFailed("build provider request".to_owned())
        })?;
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|err| io_error(&err, "send failed"))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| io_error(&err, "read failed"))?;
        // Status-class errors take priority over body shape: an error page
        // that is not JSON must still report its status, while a malformed
        // success body is a decode failure.
        let is_success = (200..300).contains(&status);
        let body: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) if is_success => {
                return Err(InferenceTechnicalError::ProviderTransportFailed(
                    "decode response body".to_owned(),
                ));
            }
            Err(_) => serde_json::Value::Null,
        };
        parse_response(status, body)
    }
}

/// Maps a request I/O failure to a technical error without secrets.
///
/// An elapsed timeout (connect or whole-request) means the call may have run,
/// so it maps to [`InferenceTechnicalError::ResponseLost`]; any other I/O
/// failure maps to a status-class transport failure.
fn io_error(err: &reqwest::Error, context: &'static str) -> InferenceTechnicalError {
    if err.is_timeout() {
        InferenceTechnicalError::ResponseLost
    } else {
        InferenceTechnicalError::ProviderTransportFailed(format!("provider unavailable: {context}"))
    }
}

/// Tolerantly decoded Responses API envelope; unknown fields are ignored.
#[derive(Deserialize)]
struct ResponsesBody {
    /// Message items; absent when the provider reports none.
    #[serde(default)]
    output: Vec<OutputItem>,
    /// Token counts; absent when the provider reports none.
    #[serde(default)]
    usage: Option<UsageObj>,
}

/// One `output` item; only its `content` parts matter here.
#[derive(Deserialize)]
struct OutputItem {
    /// Content parts; absent for items (such as reasoning) without any.
    #[serde(default)]
    content: Vec<ContentPart>,
}

/// One `output.content` part; only `output_text` parts carry `text`.
#[derive(Deserialize)]
struct ContentPart {
    /// Part discriminator, such as `"output_text"`.
    #[serde(rename = "type", default)]
    kind: Option<String>,
    /// Text carried by `output_text` parts.
    #[serde(default)]
    text: Option<String>,
}

/// Tolerantly decoded `usage` object.
#[derive(Deserialize)]
struct UsageObj {
    /// Reported input tokens.
    #[serde(default)]
    input_tokens: Option<u64>,
    /// Reported output tokens.
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// Maps one HTTP completion (`status` plus decoded JSON `body`) to a
/// [`ProviderResponse`].
///
/// Status mapping: 401 reports unauthorized (core surfaces reauthentication);
/// 429 and 5xx report provider-unavailable with the status; other non-2xx
/// reports a request failure with the status. On success, the text is the
/// concatenation of every `output[].content[]` part of type `output_text`;
/// usage is [`Some`] only when both counts are present, otherwise [`None`]
/// (core maps that to [`crate::UsageSource::Unknown`]). A success body with
/// the wrong shape reports a decode failure.
fn parse_response(
    status: u16,
    body: serde_json::Value,
) -> Result<ProviderResponse, InferenceTechnicalError> {
    if status == 401 {
        return Err(InferenceTechnicalError::ProviderTransportFailed(
            "unauthorized".to_owned(),
        ));
    }
    if status == 429 || (500..600).contains(&status) {
        return Err(InferenceTechnicalError::ProviderTransportFailed(format!(
            "provider unavailable: {status}"
        )));
    }
    if !(200..300).contains(&status) {
        return Err(InferenceTechnicalError::ProviderTransportFailed(format!(
            "provider request failed: {status}"
        )));
    }
    let decoded: ResponsesBody = serde_json::from_value(body).map_err(|_| {
        InferenceTechnicalError::ProviderTransportFailed("decode provider response".to_owned())
    })?;
    let mut text = String::new();
    for item in &decoded.output {
        for part in &item.content {
            if part.kind.as_deref() == Some("output_text")
                && let Some(piece) = &part.text
            {
                text.push_str(piece);
            }
        }
    }
    let usage =
        decoded
            .usage
            .and_then(|counts| match (counts.input_tokens, counts.output_tokens) {
                (Some(input_tokens), Some(output_tokens)) => Some(RawUsage {
                    input_tokens,
                    output_tokens,
                }),
                _ => None,
            });
    Ok(ProviderResponse { text, usage })
}

#[cfg(test)]
mod tests {
    use ene_credential::{CredentialRef, MemoryCredentialStore};

    use super::{OpenAiResponsesTransport, parse_response};
    use crate::{InferenceTechnicalError, RawUsage};

    fn error_shape() -> serde_json::Value {
        serde_json::json!({
            "error": {
                "message": "Incorrect API key provided",
                "type": "invalid_request_error",
                "code": "invalid_api_key",
            },
        })
    }

    #[test]
    fn concatenates_output_text_and_reports_usage() {
        let body = serde_json::json!({
            "id": "resp_123",
            "object": "response",
            "output": [
                {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hello, "},
                        {"type": "output_text", "text": "world!"},
                        {"type": "output_text_delta", "text": "must-not-appear"},
                        {"type": "reasoning", "summary": "must-not-appear"},
                    ],
                },
                {
                    "id": "msg_2",
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": " Again."}],
                },
            ],
            "usage": {"input_tokens": 12, "output_tokens": 5},
        });
        let result = parse_response(200, body);
        assert!(result.is_ok());
        let Ok(response) = result else {
            return;
        };
        assert_eq!(response.text, "Hello, world! Again.");
        assert_eq!(
            response.usage,
            Some(RawUsage {
                input_tokens: 12,
                output_tokens: 5,
            })
        );
    }

    #[test]
    fn missing_usage_maps_to_none() {
        let body = serde_json::json!({
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "hi"}],
                },
            ],
        });
        let result = parse_response(200, body);
        assert!(result.is_ok());
        let Ok(response) = result else {
            return;
        };
        assert_eq!(response.text, "hi");
        assert_eq!(response.usage, None);
    }

    #[test]
    fn empty_output_yields_empty_text_and_no_usage() {
        let result = parse_response(200, serde_json::json!({"output": []}));
        assert!(result.is_ok());
        let Ok(response) = result else {
            return;
        };
        assert_eq!(response.text, "");
        assert_eq!(response.usage, None);
    }

    #[test]
    fn empty_object_maps_to_empty_text_and_no_usage() {
        let result = parse_response(200, serde_json::json!({}));
        assert!(result.is_ok());
        let Ok(response) = result else {
            return;
        };
        assert_eq!(response.text, "");
        assert_eq!(response.usage, None);
    }

    #[test]
    fn null_body_maps_to_decode_failure() {
        let result = parse_response(200, serde_json::Value::Null);
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("decode"));
    }

    #[test]
    fn partial_usage_maps_to_none() {
        let body = serde_json::json!({
            "output": [],
            "usage": {"input_tokens": 7},
        });
        let result = parse_response(200, body);
        assert!(result.is_ok());
        let Ok(response) = result else {
            return;
        };
        assert_eq!(response.usage, None);
    }

    #[test]
    fn unauthorized_maps_to_transport_failure() {
        let result = parse_response(401, error_shape());
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("unauthorized"));
    }

    #[test]
    fn rate_limited_maps_to_unavailable() {
        let result = parse_response(429, error_shape());
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("provider unavailable"));
        assert!(reason.contains("429"));
    }

    #[test]
    fn server_error_maps_to_unavailable() {
        let result = parse_response(503, error_shape());
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("provider unavailable"));
        assert!(reason.contains("503"));
    }

    #[test]
    fn other_client_error_maps_to_request_failure() {
        let result = parse_response(400, error_shape());
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("provider request failed"));
        assert!(reason.contains("400"));
    }

    #[test]
    fn malformed_body_maps_to_decode_failure() {
        let body = serde_json::json!({
            "output": [{"content": "not-an-array"}],
        });
        let result = parse_response(200, body);
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let Err(InferenceTechnicalError::ProviderTransportFailed(reason)) = result else {
            return;
        };
        assert!(reason.contains("decode"));
    }

    #[test]
    fn debug_rendering_carries_no_bearer_material() {
        let concrete = MemoryCredentialStore::new();
        let credential = CredentialRef {
            id: "openai:main".to_owned(),
            provider: "openai".to_owned(),
            label: "main".to_owned(),
        };
        concrete.insert(credential.clone(), "sk-probe-bearer-material");
        let transport = OpenAiResponsesTransport::new("http://127.0.0.1:9", credential, concrete);
        let rendered = format!("{transport:?}");
        assert!(!rendered.contains("sk-probe-bearer-material"));
        assert!(!rendered.contains("Bearer"));
    }
}
