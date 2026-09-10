//! `OpenAI` Responses API transport for inference dispatch.
//!
//! [`OpenAiResponsesTransport`] posts one `{"model", "input", "stream":
//! false, "store": false}` body per [`ProviderTransport::complete`] call.
//! Key material never rests on the transport: each call borrows the bearer inside
//! [`CredentialStore::with_bearer`] and only the owned [`reqwest::Request`]
//! escapes the closure. Error strings carry status classes only, never URLs,
//! keys, or bodies. There is no retry, no streaming, and no model fallback.
//!
//! [`ProviderTransport::complete`] performs HTTPS I/O, so integration tests
//! cover it through [`crate::fake::FakeProviderTransport`]; the pure
//! `parse_response()` mapping below carries the unit tests.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ene_credential::{CredentialStore, CredentialTechnicalError};
use serde::Deserialize;

use super::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport, RawUsage,
};

/// Base URL for the `OpenAI` API; tests inject a local URL instead.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// HTTPS transport for the `OpenAI` Responses API (`POST /v1/responses`).
///
/// No field ever holds key material or a fixed credential: the bearer is
/// resolved per request from the [`ene_credential::CredentialRef`] the authorized dispatch
/// carries, and borrowed transiently inside [`CredentialStore::with_bearer`].
///
/// The store is a generic `S: CredentialStore` rather than a trait object
/// because [`CredentialStore::with_bearer`] is generic over its closure return
/// type, which makes the trait not dyn-compatible.
pub struct OpenAiResponsesTransport<S> {
    base_url: String,
    http: reqwest::Client,
    store: S,
}

impl<S> core::fmt::Debug for OpenAiResponsesTransport<S> {
    /// Renders the base URL; the HTTP client and the store render opaque so
    /// no bearer material can leak through logging.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenAiResponsesTransport")
            .field("base_url", &self.base_url)
            .field("http", &"<http-client>")
            .field("store", &"<credential-store>")
            .finish()
    }
}

impl<S: CredentialStore> OpenAiResponsesTransport<S> {
    /// The client enforces [`CONNECT_TIMEOUT`] and [`REQUEST_TIMEOUT`]. No
    /// I/O happens here; pass [`DEFAULT_BASE_URL`] for production. Each call
    /// bills the credential its [`ProviderRequest`] carries, so a consent
    /// reassignment takes effect on the next request without rebinding.
    ///
    /// # Errors
    ///
    /// Returns [`InferenceTechnicalError::HttpClientBuildFailed`] when the
    /// timeout-bound client cannot be built — the timeout invariant is
    /// reported, never silently dropped for an unbounded default.
    pub fn new(base_url: impl Into<String>, store: S) -> Result<Self, InferenceTechnicalError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| InferenceTechnicalError::HttpClientBuildFailed)?;
        Ok(Self {
            base_url: base_url.into(),
            http,
            store,
        })
    }
}

impl<S: CredentialStore> ProviderTransport for OpenAiResponsesTransport<S> {
    /// Runs one Responses API completion with no retries or side effects
    /// beyond the call.
    ///
    /// Input policy, including the length cap, is owned by the dispatch
    /// boundary ([`crate::dispatch_authorized`]); the transport bills the
    /// request's authorized credential and sends what it is given.
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + '_>>
    {
        Box::pin(self.complete_inner(req))
    }
}

impl<S: CredentialStore> OpenAiResponsesTransport<S> {
    async fn complete_inner(
        &self,
        req: ProviderRequest,
    ) -> Result<ProviderResponse, InferenceTechnicalError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/responses");
        let body = responses_body(&req.model, &req.input);
        let build = self
            .store
            .with_bearer(&req.credential, |key| {
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

/// History lives durably on the local side, which never needs server-side
/// response state; leaving `store` unset would default it to `true` and
/// retain conversation text provider-side for no reason. This is a
/// storage-scope boundary, not a no-logging promise: it disables the
/// Responses application-state store, nothing more.
fn responses_body(model: &str, input: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": input,
        "stream": false,
        "store": false,
    })
}

/// An elapsed timeout (connect or whole-request) may mean the call ran, so it
/// maps to [`InferenceTechnicalError::ResponseLost`]; any other I/O failure
/// maps to a status-class transport failure that carries no secrets.
fn io_error(err: &reqwest::Error, context: &'static str) -> InferenceTechnicalError {
    if err.is_timeout() {
        InferenceTechnicalError::ResponseLost
    } else {
        InferenceTechnicalError::ProviderTransportFailed(format!("provider unavailable: {context}"))
    }
}

/// Tolerantly decoded Responses API envelope; unknown fields are ignored.
///
/// `status` has no default: a response without one is a protocol failure,
/// never a silent success (see [`parse_response`]).
#[derive(Deserialize)]
struct ResponsesBody {
    status: Option<String>,
    #[serde(default)]
    output: Vec<OutputItem>,
    #[serde(default)]
    usage: Option<UsageObj>,
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(default)]
    content: Vec<ContentPart>,
}

#[derive(Deserialize)]
struct ContentPart {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct UsageObj {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// Maps one HTTP completion (`status` plus decoded JSON `body`) to a
/// [`ProviderResponse`].
///
/// Status mapping: 401 reports unauthorized (core surfaces reauthentication);
/// 429 and 5xx report provider-unavailable with the status; other non-2xx
/// reports a request failure with the status. On 2xx, only a response
/// object with `status == "completed"` succeeds: `incomplete` reports the
/// bounded reason class, `failed` / `cancelled` report their status, and
/// `queued` / `in_progress` / unknown / missing statuses report an
/// unexpected-state failure — this synchronous contract never waits on a
/// background response. On success, the text is the concatenation of every
/// `output[].content[]` part of type `output_text`; usage is [`Some`] only
/// when both counts are present, otherwise [`None`] (core maps that to
/// [`crate::UsageSource::Unknown`]). A success body with the wrong shape
/// reports a decode failure. Reason strings carry only the bounded
/// `incomplete_details.reason` vocabulary, never message bodies.
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
    match decoded.status.as_deref() {
        Some("completed") => {}
        Some("incomplete") => {
            let reason = decoded
                .incomplete_details
                .as_ref()
                .and_then(|details| details.reason.clone())
                .unwrap_or_else(|| String::from("unknown"));
            return Err(InferenceTechnicalError::ProviderTransportFailed(format!(
                "provider response incomplete: {reason}"
            )));
        }
        Some("failed") => {
            return Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider response failed".to_owned(),
            ));
        }
        Some("cancelled") => {
            return Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider response cancelled".to_owned(),
            ));
        }
        Some(other) => {
            return Err(InferenceTechnicalError::ProviderTransportFailed(format!(
                "provider response unexpected state: {other}"
            )));
        }
        None => {
            return Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider response missing status".to_owned(),
            ));
        }
    }
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
            "status": "completed",
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
        let response = result.unwrap();
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
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "hi"}],
                },
            ],
        });
        let result = parse_response(200, body);
        let response = result.unwrap();
        assert_eq!(response.text, "hi");
        assert_eq!(response.usage, None);
    }

    #[test]
    fn request_body_disables_server_side_storage() {
        let body = super::responses_body("gpt-test", "hello");
        assert_eq!(
            body.get("store"),
            Some(&serde_json::Value::Bool(false)),
            "history lives locally; the provider must not retain response state: {body}"
        );
        assert_eq!(body.get("stream"), Some(&serde_json::Value::Bool(false)));
    }

    #[test]
    fn empty_output_without_status_is_not_success() {
        let result = parse_response(200, serde_json::json!({"output": []}));
        assert!(
            matches!(
                result,
                Err(InferenceTechnicalError::ProviderTransportFailed(_))
            ),
            "a status-less body must fail, got {result:?}"
        );
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
        };
        assert!(reason.contains("missing status"), "got {reason:?}");
    }

    #[test]
    fn non_completed_statuses_fail_without_text_or_usage() {
        for (status, body, marker) in [
            (
                "incomplete",
                serde_json::json!({
                    "status": "incomplete",
                    "incomplete_details": {"reason": "max_output_tokens"},
                    "output": [{"content": [{"type": "output_text", "text": "partial"}]}],
                }),
                "incomplete",
            ),
            (
                "incomplete without reason",
                serde_json::json!({"status": "incomplete"}),
                "incomplete",
            ),
            (
                "failed",
                serde_json::json!({"status": "failed", "error": {"message": "boom"}}),
                "failed",
            ),
            (
                "cancelled",
                serde_json::json!({"status": "cancelled"}),
                "cancelled",
            ),
            (
                "queued",
                serde_json::json!({"status": "queued"}),
                "unexpected state",
            ),
            (
                "in_progress",
                serde_json::json!({"status": "in_progress"}),
                "unexpected state",
            ),
            (
                "unknown future status",
                serde_json::json!({"status": "super_completed"}),
                "unexpected state",
            ),
        ] {
            let result = parse_response(200, body);
            assert!(
                matches!(
                    result,
                    Err(InferenceTechnicalError::ProviderTransportFailed(_))
                ),
                "{status} must fail, got {result:?}"
            );
            let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err()
            else {
                panic!("unexpected variant");
            };
            assert!(
                reason.contains(marker),
                "{status} must report its class, got {reason:?}"
            );
            assert!(
                !reason.contains("partial") && !reason.contains("boom"),
                "{status} must not leak body text, got {reason:?}"
            );
        }
    }

    #[test]
    fn null_body_maps_to_decode_failure() {
        let result = parse_response(200, serde_json::Value::Null);
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
        };
        assert!(reason.contains("decode"));
    }

    #[test]
    fn partial_usage_maps_to_none() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [],
            "usage": {"input_tokens": 7},
        });
        let result = parse_response(200, body);
        let response = result.unwrap();
        assert_eq!(response.usage, None);
    }

    #[test]
    fn unauthorized_maps_to_transport_failure() {
        let result = parse_response(401, error_shape());
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
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
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
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
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
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
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
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
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
        };
        assert!(reason.contains("decode"));
    }

    #[test]
    fn debug_rendering_carries_no_bearer_material() {
        let concrete = MemoryCredentialStore::new();
        let credential = CredentialRef::new("openai", "main").expect("valid test fixture");
        concrete.insert(credential.clone(), "sk-probe-bearer-material");
        let transport = OpenAiResponsesTransport::new("http://127.0.0.1:9", concrete).unwrap();
        let rendered = format!("{transport:?}");
        assert!(!rendered.contains("sk-probe-bearer-material"));
        assert!(!rendered.contains("Bearer"));
    }
}
