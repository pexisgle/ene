//! `OpenAI` Responses API transport for inference dispatch.
//!
//! [`OpenAiResponsesTransport`] posts one
//! `{"model", "input", "stream": true, "store": false, "max_output_tokens"}`
//! body per [`ProviderTransport::complete_streaming`] call and parses the
//! server-sent event stream, forwarding text deltas as they arrive;
//! [`ProviderTransport::complete`] keeps the non-streaming JSON path.
//! Key material never rests on the transport: each call borrows the bearer inside
//! [`CredentialStore::with_bearer`] and only the owned [`reqwest::Request`]
//! escapes the closure. Error strings carry status classes only, never URLs,
//! keys, or bodies. There is no retry and no model fallback.
//!
//! Both paths perform HTTPS I/O, so integration tests cover them through
//! transport fakes; the pure `parse_response()` and `StreamAssembler`
//! mappings below carry the unit tests.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ene_credential::{CredentialStore, CredentialTechnicalError};
use serde::Deserialize;

use super::{
    DeltaFlow, DeltaSink, InferenceTechnicalError, ProviderRequest, ProviderResponse,
    ProviderTransport, RawUsage, UsageEstimate,
};

/// Base URL for the `OpenAI` API; tests inject a local URL instead.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Explicit output maximum sent with every Responses request.
///
/// `usage-cost-cap` §8 requires the output side of the reservation upper bound
/// to be the request's own explicit maximum, not a prediction. The body and
/// [`OpenAiResponsesTransport::usage_estimate`] share this constant, so the
/// bound can never drift below what the provider is allowed to generate.
pub const MAX_OUTPUT_TOKENS: u64 = 4_096;

/// Protocol-framing allowance added to the request text's byte length for the
/// input side of the reservation upper bound.
///
/// A BPE tokenizer never emits fewer than one token per byte for the text
/// itself, so the UTF-8 byte length is already a tokenizer-safe bound for the
/// input string; the allowance covers server-side framing (special tokens and
/// request formatting) that the local body does not spell out.
pub const INPUT_TOKENS_FRAMING_ALLOWANCE: u64 = 1_024;

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

    /// The safe upper bound of one Responses request.
    ///
    /// The input side is the request text's UTF-8 byte length plus
    /// [`INPUT_TOKENS_FRAMING_ALLOWANCE`]; the output side is the explicit
    /// [`MAX_OUTPUT_TOKENS`] the body carries. Both are contract bounds, not
    /// estimates of what this prompt will use.
    fn usage_estimate(&self, req: &ProviderRequest) -> Option<UsageEstimate> {
        let input_bytes = u64::try_from(req.input.len()).ok()?;
        Some(UsageEstimate {
            input_tokens_upper_bound: input_bytes.checked_add(INPUT_TOKENS_FRAMING_ALLOWANCE)?,
            output_tokens_upper_bound: MAX_OUTPUT_TOKENS,
        })
    }

    /// Runs one Responses API completion with `"stream": true`, forwarding
    /// each `response.output_text.delta` as it arrives.
    ///
    /// The returned response still carries the full assembled text and usage,
    /// so adoption, durable History, and display remain separate facts. When
    /// the sink aborts, the SSE read stops with it: no later delta is
    /// presented, and the call never completes normally with a gap.
    fn complete_streaming<'a>(
        &'a self,
        req: ProviderRequest,
        sink: &'a mut (dyn DeltaSink + Send),
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + 'a>>
    {
        Box::pin(self.complete_streaming_inner(req, sink))
    }
}

impl<S: CredentialStore> OpenAiResponsesTransport<S> {
    async fn complete_inner(
        &self,
        req: ProviderRequest,
    ) -> Result<ProviderResponse, InferenceTechnicalError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/responses");
        let body = responses_body(&req.model, &req.input, false);
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

    /// Streaming sibling of [`Self::complete_inner`]: status errors fall back
    /// to the same status-class mapping (an error body is not an event
    /// stream), while a 2xx body is parsed as server-sent events.
    async fn complete_streaming_inner(
        &self,
        req: ProviderRequest,
        sink: &mut (dyn DeltaSink + Send),
    ) -> Result<ProviderResponse, InferenceTechnicalError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/responses");
        let body = responses_body(&req.model, &req.input, true);
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
        let mut response = self
            .http
            .execute(request)
            .await
            .map_err(|err| io_error(&err, "send failed"))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            // Status-class errors take priority over stream shape: an error
            // body is JSON, not events, and must report its status.
            let bytes = response
                .bytes()
                .await
                .map_err(|err| io_error(&err, "read failed"))?;
            let body: serde_json::Value =
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            return parse_response(status, body);
        }

        let mut assembler = StreamAssembler::default();
        let mut pending = Vec::<u8>::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| io_error(&err, "read failed"))?
        {
            pending.extend_from_slice(&chunk);
            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let raw: Vec<u8> = pending.drain(..=newline).collect();
                let line = String::from_utf8_lossy(&raw);
                if let Some(delta) = assembler.feed_line(line.trim_end_matches(['\r', '\n']))?
                    && let DeltaFlow::Abort(reason) = sink.push_delta(&delta).await
                {
                    return Err(InferenceTechnicalError::StreamAborted {
                        reason: reason.to_owned(),
                    });
                }
            }
        }
        assembler.finish()
    }
}

/// Incremental parser for the Responses API event stream.
///
/// Only the events this stage consumes are interpreted: text deltas,
/// completion with usage, and bounded failure events. Unknown event types are
/// ignored (forward compatibility), while malformed JSON or a missing
/// completion is a decode failure, never a silent success. Error strings
/// carry the event class only, never provider body text.
#[derive(Default)]
struct StreamAssembler {
    text: String,
    usage: Option<RawUsage>,
    completed: bool,
}

impl StreamAssembler {
    /// Feeds one event-stream line, returning the text delta it carries, if
    /// any. The caller pushes the delta to its sink: parsing stays a pure
    /// sync mapping with no delivery policy of its own.
    fn feed_line(&mut self, line: &str) -> Result<Option<String>, InferenceTechnicalError> {
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(None);
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }
        let event: serde_json::Value = serde_json::from_str(data).map_err(|_| {
            InferenceTechnicalError::ProviderTransportFailed("decode provider stream".to_owned())
        })?;
        match event.get("type").and_then(|kind| kind.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(|delta| delta.as_str()) {
                    self.text.push_str(delta);
                    return Ok(Some(delta.to_owned()));
                }
                Ok(None)
            }
            Some("response.completed") => {
                self.usage = event
                    .get("response")
                    .and_then(|response| response.get("usage"))
                    .and_then(|usage| serde_json::from_value::<UsageObj>(usage.clone()).ok())
                    .and_then(UsageObj::into_raw);
                self.completed = true;
                Ok(None)
            }
            Some("response.incomplete") => {
                let reason = event
                    .get("response")
                    .and_then(|response| response.get("incomplete_details"))
                    .and_then(|details| details.get("reason"))
                    .and_then(|reason| reason.as_str())
                    .unwrap_or("unknown");
                Err(InferenceTechnicalError::ProviderTransportFailed(format!(
                    "provider response incomplete: {reason}"
                )))
            }
            Some("response.failed") => Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider response failed".to_owned(),
            )),
            Some("error") => Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider stream error".to_owned(),
            )),
            _ => Ok(None),
        }
    }

    fn finish(self) -> Result<ProviderResponse, InferenceTechnicalError> {
        if !self.completed {
            return Err(InferenceTechnicalError::ProviderTransportFailed(
                "provider stream ended before completion".to_owned(),
            ));
        }
        Ok(ProviderResponse {
            text: self.text,
            usage: self.usage,
        })
    }
}

/// History lives durably on the local side, which never needs server-side
/// response state; leaving `store` unset would default it to `true` and
/// retain conversation text provider-side for no reason. This is a
/// storage-scope boundary, not a no-logging promise: it disables the
/// Responses application-state store, nothing more. `max_output_tokens` is
/// the explicit maximum the reservation upper bound uses, so the provider
/// cannot generate more output than the bound covers.
fn responses_body(model: &str, input: &str, stream: bool) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": input,
        "stream": stream,
        "store": false,
        "max_output_tokens": MAX_OUTPUT_TOKENS,
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
    usage: Option<serde_json::Value>,
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
    #[serde(default)]
    input_tokens_details: Option<InputTokenDetails>,
}

#[derive(Deserialize)]
struct InputTokenDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

impl UsageObj {
    /// Only a complete, valid report is known. Absent cache detail is not
    /// evidence of zero cache hits, even when input and output are present.
    fn into_raw(self) -> Option<RawUsage> {
        let input_tokens = self.input_tokens?;
        let output_tokens = self.output_tokens?;
        let cached_input_tokens = self.input_tokens_details?.cached_tokens?;
        (cached_input_tokens <= input_tokens).then_some(RawUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens,
        })
    }
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
    let usage = decoded
        .usage
        .and_then(|usage| serde_json::from_value::<UsageObj>(usage).ok())
        .and_then(UsageObj::into_raw);
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
            "usage": {"input_tokens": 12, "output_tokens": 5, "input_tokens_details": {"cached_tokens": 1}},
        });
        let result = parse_response(200, body);
        let response = result.unwrap();
        assert_eq!(response.text, "Hello, world! Again.");
        assert_eq!(
            response.usage,
            Some(RawUsage {
                input_tokens: 12,
                cached_input_tokens: 1,
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
        let body = super::responses_body("gpt-test", "hello", false);
        assert_eq!(
            body.get("store"),
            Some(&serde_json::Value::Bool(false)),
            "history lives locally; the provider must not retain response state: {body}"
        );
        assert_eq!(body.get("stream"), Some(&serde_json::Value::Bool(false)));
    }

    #[test]
    fn request_body_sets_the_explicit_output_maximum_the_estimate_uses() {
        let body = super::responses_body("gpt-test", "hello", false);
        assert_eq!(
            body.get("max_output_tokens"),
            Some(&serde_json::json!(super::MAX_OUTPUT_TOKENS)),
            "the request itself must carry the explicit maximum the reservation bound covers"
        );
    }

    #[test]
    fn usage_estimate_bounds_input_by_bytes_plus_framing_and_output_by_the_request_maximum() {
        let transport = OpenAiResponsesTransport::new(
            super::DEFAULT_BASE_URL,
            MemoryCredentialStore::default(),
        )
        .expect("the test transport builds");
        let request = crate::ProviderRequest {
            model: String::from("gpt-test"),
            credential: CredentialRef::new(String::from("openai"), String::from("main"))
                .expect("valid test fixture"),
            input: String::from("hello"),
        };
        let estimate = crate::ProviderTransport::usage_estimate(&transport, &request)
            .expect("the OpenAI adapter always has a finite bound");
        assert_eq!(
            estimate.input_tokens_upper_bound,
            5 + super::INPUT_TOKENS_FRAMING_ALLOWANCE,
            "the text's UTF-8 byte length is the tokenizer-safe lower bound, plus framing"
        );
        assert_eq!(
            estimate.output_tokens_upper_bound,
            super::MAX_OUTPUT_TOKENS,
            "the output side must be the explicit maximum the body carries"
        );
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
    fn missing_cache_detail_is_unknown_not_zero() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [],
            "usage": {"input_tokens": 7, "output_tokens": 3},
        });
        let result = parse_response(200, body);
        let response = result.unwrap();
        assert_eq!(response.text, "");
        assert_eq!(
            response.usage, None,
            "cache detail absence is not evidence of zero cache hits"
        );
    }

    #[test]
    fn cache_detail_absent_and_empty_both_stay_unknown() {
        // Explicit null and an empty details object carry no cached count.
        for usage in [
            serde_json::json!({"input_tokens": 7, "output_tokens": 3, "input_tokens_details": null}),
            serde_json::json!({"input_tokens": 7, "output_tokens": 3, "input_tokens_details": {}}),
        ] {
            let body = serde_json::json!({
                "status": "completed",
                "output": [],
                "usage": usage,
            });
            let result = parse_response(200, body);
            let response = result.unwrap();
            assert_eq!(
                response.usage, None,
                "a cache detail without cached_tokens is not a zero cache"
            );
        }
    }

    #[test]
    fn cache_count_above_input_is_rejected_as_unknown() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "input_tokens_details": {"cached_tokens": 6},
            },
        });
        let result = parse_response(200, body);
        let response = result.unwrap();
        assert_eq!(
            response.usage, None,
            "cached subset larger than input is not a correct usage report"
        );
    }

    #[test]
    fn malformed_usage_json_keeps_the_valid_response_unknown() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "hi"}]}],
            "usage": {"input_tokens": "lots"},
        });
        let result = parse_response(200, body);
        let response = result.expect("the response envelope itself is valid");
        assert_eq!(response.text, "hi");
        assert_eq!(
            response.usage, None,
            "an undecodable usage report settles Unknown, not a failed call"
        );
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
    fn stream_assembler_missing_cache_detail_reports_unknown_usage() {
        let mut assembler = super::StreamAssembler::default();
        for line in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}",
        ] {
            assembler.feed_line(line).expect("a known event must parse");
        }
        let response = assembler.finish().expect("a completed stream answers");
        assert_eq!(response.text, "hi");
        assert_eq!(
            response.usage, None,
            "SSE completion without cache detail settles Unknown, never zero"
        );
    }

    #[test]
    fn streaming_body_requests_incremental_output() {
        let body = super::responses_body("gpt-test", "hello", true);
        assert_eq!(
            body.get("stream"),
            Some(&serde_json::Value::Bool(true)),
            "the streaming transport must request server-sent events: {body}"
        );
        assert_eq!(body.get("store"), Some(&serde_json::Value::Bool(false)));
    }

    #[test]
    fn stream_assembler_emits_deltas_in_order_and_reports_usage() {
        let mut assembler = super::StreamAssembler::default();
        let mut deltas = Vec::new();
        for line in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}",
            "event: response.output_text.delta",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":1}}}}",
        ] {
            let fed = assembler.feed_line(line).expect("a known event must parse");
            deltas.extend(fed);
        }
        assert_eq!(deltas, vec!["Hel", "lo"]);
        let response = assembler.finish().expect("the stream completed");
        assert_eq!(response.text, "Hello");
        assert_eq!(
            response.usage,
            Some(crate::RawUsage {
                input_tokens: 7,
                cached_input_tokens: 1,
                output_tokens: 3,
            })
        );
    }

    #[test]
    fn stream_assembler_treats_missing_completion_as_failure() {
        let mut assembler = super::StreamAssembler::default();
        let fed = assembler
            .feed_line("data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}")
            .expect("the delta parses");
        assert_eq!(fed.as_deref(), Some("partial"));
        let result = assembler.finish();
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::ProviderTransportFailed(_))
        ));
        let InferenceTechnicalError::ProviderTransportFailed(reason) = result.unwrap_err() else {
            panic!("unexpected variant");
        };
        assert!(reason.contains("completion"), "got {reason:?}");
    }

    #[test]
    fn stream_assembler_maps_failure_events_without_body_text() {
        for (line, marker) in [
            (
                "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}",
                "incomplete",
            ),
            ("data: {\"type\":\"response.failed\"}", "failed"),
            (
                "data: {\"type\":\"error\",\"message\":\"secret body text\"}",
                "error",
            ),
        ] {
            let mut assembler = super::StreamAssembler::default();
            let result = assembler.feed_line(line);
            let InferenceTechnicalError::ProviderTransportFailed(reason) =
                result.expect_err("failure events must fail")
            else {
                panic!("unexpected variant");
            };
            assert!(reason.contains(marker), "got {reason:?}");
            assert!(
                !reason.contains("secret body text"),
                "failure reasons must not echo provider body text: {reason:?}"
            );
        }
    }

    #[test]
    fn stream_assembler_ignores_unknown_events_and_malformed_lines() {
        let mut assembler = super::StreamAssembler::default();
        let mut deltas = Vec::new();
        deltas.extend(
            assembler
                .feed_line("event: response.output_text.delta")
                .expect("a non-data line is ignored"),
        );
        deltas.extend(
            assembler
                .feed_line("data: {\"type\":\"response.future_event\",\"x\":1}")
                .expect("an unknown event is ignored"),
        );
        assert!(deltas.is_empty());
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

    /// Models the pinned credential set the real Host wires into the
    /// scrubber: the ref list and the pinned value agree on one pair.
    struct FixtureRefs {
        refs: Vec<CredentialRef>,
        revision: ene_credential::CredentialSetRevision,
    }

    impl ene_credential::CredentialRefRepository for FixtureRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn list_refs(
            &self,
        ) -> Result<Vec<CredentialRef>, ene_credential::CredentialTechnicalError> {
            Ok(self.refs.clone())
        }
    }

    impl ene_credential::CredentialSetRepository for FixtureRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn current_set_revision(
            &self,
        ) -> Result<ene_credential::CredentialSetRevision, ene_credential::CredentialTechnicalError>
        {
            Ok(self.revision)
        }
    }

    /// Serves one HTTP/1.1 response with `status` and `payload`, recording the
    /// exact request bytes (head and body) the production transport emitted.
    async fn spawn_capturing_responses(
        status: u16,
        payload: String,
    ) -> Option<(
        std::net::SocketAddr,
        tokio::task::JoinHandle<()>,
        std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    )> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
        let addr = listener.local_addr().ok()?;
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = std::sync::Arc::clone(&captured);
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if head.len() > 16_384 {
                    return;
                }
                match stream.read(&mut byte).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => head.push(byte[0]),
                }
            }
            let head_text = String::from_utf8_lossy(&head).into_owned();
            let mut content_length = 0_usize;
            for line in head_text.lines().skip(1) {
                let Some((name, value)) = line.split_once(':') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("content-length")
                    && let Ok(parsed) = value.trim().parse::<usize>()
                {
                    content_length = parsed;
                }
            }
            let mut body = vec![0_u8; content_length.min(1_048_576)];
            let mut filled = 0_usize;
            while filled < body.len() {
                match stream.read(&mut body[filled..]).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => filled += read,
                }
            }
            record
                .lock()
                .expect("capture lock")
                .push((head_text, String::from_utf8_lossy(&body).into_owned()));
            let reason = if status == 200 {
                "OK"
            } else {
                "Internal Server Error"
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
                payload.len()
            );
            drop(stream.write_all(response.as_bytes()).await);
            drop(stream.shutdown().await);
        });
        Some((addr, handle, captured))
    }

    /// The HTTP authorization boundary is the only place the pinned bearer may
    /// appear: the body carries the scrubbed input, and no debug rendering of
    /// the transport or the request carries the value.
    #[tokio::test]
    async fn bearer_stays_at_the_http_authorization_boundary() {
        use crate::ProviderTransport as _;
        use ene_credential::{CredentialScrubber, CredentialSetRevision, SecretScrubber as _};

        let secret = "sk-transport-boundary-probe";
        let credential = CredentialRef::new("openai", "main").expect("valid test fixture");
        let concrete = MemoryCredentialStore::new();
        concrete.insert(credential.clone(), secret);
        let refs = FixtureRefs {
            refs: vec![credential.clone()],
            revision: CredentialSetRevision::from_u64(4),
        };
        let proof = CredentialScrubber {
            refs: &refs,
            store: &concrete,
        }
        .scrub(&format!("the owner key is {secret}"))
        .await
        .expect("the fixture registry is readable");
        assert!(
            !proof.text().contains(secret),
            "the proof must not carry the registered value"
        );
        assert!(proof.text().contains("[credential]"));

        let completed = r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}]}"#;
        let (addr, server, captured) = spawn_capturing_responses(200, completed.to_owned())
            .await
            .expect("the local listener must bind");
        let transport =
            OpenAiResponsesTransport::new(format!("http://{addr}"), concrete).expect("client");
        let request = crate::ProviderRequest {
            model: String::from("gpt-test"),
            credential,
            input: proof.into_text(),
        };
        assert!(
            !format!("{request:?}").contains(secret),
            "request debug must not carry the bearer"
        );
        let response = transport
            .complete(request)
            .await
            .expect("the completion must answer");
        assert_eq!(response.text, "ok");
        assert!(
            !format!("{transport:?}").contains(secret),
            "transport debug must not carry the bearer"
        );
        server.abort();

        let requests = captured.lock().expect("capture lock").clone();
        assert_eq!(requests.len(), 1, "exactly one request must be observed");
        let (head, body) = &requests[0];
        let auth_lines: Vec<&str> = head
            .lines()
            .filter(|line| {
                line.split_once(':')
                    .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
            })
            .collect();
        assert_eq!(auth_lines.len(), 1, "the bearer travels in one auth header");
        assert!(
            auth_lines[0].contains(secret),
            "the positive control: the pinned bearer is the header value"
        );
        let head_without_auth = head
            .lines()
            .filter(|line| !auth_lines.contains(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !head_without_auth.contains(secret),
            "no header other than authorization may carry the bearer: {head_without_auth}"
        );
        assert!(
            !body.contains(secret),
            "the body must carry only the scrubbed input: {body}"
        );
        assert!(
            body.contains("[credential]"),
            "the body carries the redacted position: {body}"
        );
        assert!(
            body.contains("\"store\":false"),
            "the request keeps server-side storage disabled: {body}"
        );

        // A provider failure that echoes the value in its error body and a
        // transport failure at a closed port both render their class only.
        let failing_store = MemoryCredentialStore::new();
        failing_store.insert(
            CredentialRef::new("openai", "main").expect("valid test fixture"),
            secret,
        );
        let error_body = format!(r#"{{"error":{{"message":"the key {secret} is invalid"}}}}"#);
        let (error_addr, error_server, _) = spawn_capturing_responses(500, error_body)
            .await
            .expect("the local listener must bind");
        let failing = OpenAiResponsesTransport::new(format!("http://{error_addr}"), failing_store)
            .expect("client");
        let error = failing
            .complete(crate::ProviderRequest {
                model: String::from("gpt-test"),
                credential: CredentialRef::new("openai", "main").expect("valid test fixture"),
                input: String::from("the owner key is [credential]"),
            })
            .await
            .expect_err("a 500 must fail");
        assert!(
            !error.to_string().contains(secret) && !format!("{error:?}").contains(secret),
            "a provider error must not echo the body value: {error:?}"
        );
        error_server.abort();

        let unreachable =
            OpenAiResponsesTransport::new("http://127.0.0.1:1", MemoryCredentialStore::new())
                .expect("client");
        let error = unreachable
            .complete(crate::ProviderRequest {
                model: String::from("gpt-test"),
                credential: CredentialRef::new("openai", "main").expect("valid test fixture"),
                input: String::from("the owner key is [credential]"),
            })
            .await
            .expect_err("a closed port must fail");
        assert!(
            !error.to_string().contains(secret) && !format!("{error:?}").contains(secret),
            "a transport failure must not carry the bearer: {error:?}"
        );
    }
}
