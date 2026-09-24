use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ene_credential::{CredentialRef, CredentialStore, CredentialTechnicalError};
use serde::Deserialize;

use super::{
    DeltaFlow, DeltaSink, InferenceTechnicalError, ProviderRequest, ProviderResponse,
    ProviderTransport, RawUsage, UsageEstimate,
};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub const MAX_OUTPUT_TOKENS: u64 = 4_096;

pub const INPUT_TOKENS_FRAMING_ALLOWANCE: u64 = 1_024;

pub const MAX_PROVIDER_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

pub struct OpenAiResponsesTransport<S> {
    base_url: String,
    http: reqwest::Client,
    store: S,
}

impl<S> core::fmt::Debug for OpenAiResponsesTransport<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenAiResponsesTransport")
            .field("base_url", &self.base_url)
            .field("http", &"<http-client>")
            .field("store", &"<credential-store>")
            .finish()
    }
}

impl<S: CredentialStore> OpenAiResponsesTransport<S> {
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
    fn usage_estimate(&self, req: &ProviderRequest) -> Option<UsageEstimate> {
        let input_bytes = u64::try_from(req.input.len()).ok()?;
        Some(UsageEstimate {
            input_tokens_upper_bound: input_bytes.checked_add(INPUT_TOKENS_FRAMING_ALLOWANCE)?,
            output_tokens_upper_bound: MAX_OUTPUT_TOKENS,
        })
    }

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
    async fn send_responses_request(
        &self,
        model: &str,
        input: &str,
        credential: &CredentialRef,
    ) -> Result<reqwest::Response, InferenceTechnicalError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/responses");
        let body = responses_body(model, input);
        let build = self
            .store
            .with_bearer(credential, |key| {
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
        self.http
            .execute(request)
            .await
            .map_err(|err| io_error(&err, "send failed"))
    }

    async fn complete_streaming_inner(
        &self,
        req: ProviderRequest,
        sink: &mut (dyn DeltaSink + Send),
    ) -> Result<ProviderResponse, InferenceTechnicalError> {
        let mut response = self
            .send_responses_request(&req.model, &req.input, &req.credential)
            .await?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(status_failure(status));
        }

        let mut assembler = StreamAssembler::default();
        let mut pending = Vec::<u8>::new();
        let mut received = 0_usize;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| io_error(&err, "read failed"))?
        {
            pending.extend_from_slice(&chunk);
            received = received.saturating_add(chunk.len());
            if received > MAX_PROVIDER_RESPONSE_BYTES {
                return Err(InferenceTechnicalError::ProviderTransportFailed(
                    "provider response too large".to_owned(),
                ));
            }
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

#[derive(Default)]
struct StreamAssembler {
    text: String,
    usage: Option<RawUsage>,
    completed: bool,
}

impl StreamAssembler {
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
                Err(InferenceTechnicalError::ProviderTransportFailed(
                    "decode provider stream".to_owned(),
                ))
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

fn responses_body(model: &str, input: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
        "max_output_tokens": MAX_OUTPUT_TOKENS,
    })
}

fn io_error(err: &reqwest::Error, context: &'static str) -> InferenceTechnicalError {
    if err.is_timeout() {
        InferenceTechnicalError::ResponseLost
    } else {
        InferenceTechnicalError::ProviderTransportFailed(format!("provider unavailable: {context}"))
    }
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

fn status_failure(status: u16) -> InferenceTechnicalError {
    if status == 401 {
        return InferenceTechnicalError::ProviderTransportFailed("unauthorized".to_owned());
    }
    if status == 429 || (500..600).contains(&status) {
        return InferenceTechnicalError::ProviderTransportFailed(format!(
            "provider unavailable: {status}"
        ));
    }
    InferenceTechnicalError::ProviderTransportFailed(format!("provider request failed: {status}"))
}

#[cfg(test)]
mod tests {
    use ene_credential::{CredentialRef, MemoryCredentialStore};

    use super::OpenAiResponsesTransport;
    use crate::InferenceTechnicalError;

    #[test]
    fn request_and_estimate_share_conservative_output_cap() {
        let body = super::responses_body("gpt-test", "hello");
        assert_eq!(body.get("stream"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(body.get("store"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(
            body.get("max_output_tokens"),
            Some(&serde_json::json!(super::MAX_OUTPUT_TOKENS))
        );

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
            5 + super::INPUT_TOKENS_FRAMING_ALLOWANCE
        );
        assert_eq!(estimate.output_tokens_upper_bound, super::MAX_OUTPUT_TOKENS);
    }

    #[test]
    fn stream_assembler_orders_deltas_and_requires_complete_usage() {
        let mut assembler = super::StreamAssembler::default();
        let mut deltas = Vec::new();
        for line in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}",
            "event: response.output_text.delta",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":1}}}}",
        ] {
            deltas.extend(assembler.feed_line(line).expect("a known event parses"));
        }
        assert_eq!(deltas, ["Hel", "lo"]);
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

        let mut assembler = super::StreamAssembler::default();
        for line in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}",
        ] {
            assembler.feed_line(line).expect("a known event parses");
        }
        let response = assembler.finish().expect("a completed stream answers");
        assert_eq!(response.text, "hi");
        assert_eq!(response.usage, None);
    }

    #[test]
    fn stream_assembler_fails_closed_on_incomplete_or_malformed_events() {
        let mut assembler = super::StreamAssembler::default();
        let fed = assembler
            .feed_line("data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}")
            .expect("the delta parses");
        assert_eq!(fed.as_deref(), Some("partial"));
        let InferenceTechnicalError::ProviderTransportFailed(reason) =
            assembler.finish().expect_err("missing completion fails")
        else {
            panic!("unexpected variant");
        };
        assert!(reason.contains("completion"));

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
            let InferenceTechnicalError::ProviderTransportFailed(reason) =
                assembler.feed_line(line).expect_err("failure events fail")
            else {
                panic!("unexpected variant");
            };
            assert!(reason.contains(marker));
            assert!(!reason.contains("secret body text"));
        }

        for line in [
            "data: {\"type\":\"response.output_text.delta\"}",
            "data: {not-json",
        ] {
            let mut assembler = super::StreamAssembler::default();
            assert!(matches!(
                assembler.feed_line(line),
                Err(InferenceTechnicalError::ProviderTransportFailed(_))
            ));
        }
    }

    #[test]
    fn stream_assembler_ignores_non_data_lines_and_future_events() {
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

        let completed = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\ndata: {\"type\":\"response.completed\"}\n\n";
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
            .complete_streaming(request, &mut crate::DiscardSink)
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
        let (name, value) = auth_lines[0]
            .split_once(':')
            .expect("the authorization header has a value");
        assert_eq!(name.trim().to_ascii_lowercase(), "authorization");
        assert_eq!(value.trim(), format!("Bearer {secret}"));
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
            .complete_streaming(
                crate::ProviderRequest {
                    model: String::from("gpt-test"),
                    credential: CredentialRef::new("openai", "main").expect("valid test fixture"),
                    input: String::from("the owner key is [credential]"),
                },
                &mut crate::DiscardSink,
            )
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
            .complete_streaming(
                crate::ProviderRequest {
                    model: String::from("gpt-test"),
                    credential: CredentialRef::new("openai", "main").expect("valid test fixture"),
                    input: String::from("the owner key is [credential]"),
                },
                &mut crate::DiscardSink,
            )
            .await
            .expect_err("a closed port must fail");
        assert!(
            !error.to_string().contains(secret) && !format!("{error:?}").contains(secret),
            "a transport failure must not carry the bearer: {error:?}"
        );
    }
}
