//! `OpenAI Speech API` HTTP client: request building, bounded streaming
//! reads, and typed error mapping.

use std::sync::OnceLock;
use std::time::Duration;

use ene_plugin::{PluginError, ProviderErrorKind};
use reqwest::StatusCode;
use serde::Serialize;
use tokio_stream::StreamExt;

use crate::config::OpenAiTtsConfig;
use crate::pcm;
use crate::wav;

/// Timeout for establishing an HTTP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Total timeout for a single synthesis request (covers streamed bodies).
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// Retry budget for transient upstream failures (429 / network); the host
/// TTS consumer never retries, so the plugin absorbs them like the openai
/// plugin does.
const MAX_ATTEMPTS: u32 = 3;
/// Base backoff for retry attempts, doubled per attempt and jittered.
const BASE_DELAY: Duration = Duration::from_millis(500);
/// Upper bound on any single backoff (including a server `Retry-After`).
const MAX_DELAY: Duration = Duration::from_secs(30);
/// Cap on the raw PCM body. 24 kHz s16 mono is ~2.9 MB per minute, so
/// 32 MiB covers very long utterances while bounding the memory a
/// misbehaving upstream can make the plugin allocate. The 44-byte WAV
/// header keeps the wrapped payload at or under the host adapter's
/// `MAX_WAV_BYTES` cap.
pub(crate) const MAX_PCM_BYTES: usize = 32 * 1024 * 1024 - 44;
/// Cap on error-response bodies; only a snippet is surfaced anyway.
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Shared HTTP client, built once with timeouts and reused for all requests.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Speech API request body. `response_format` is fixed to `pcm`: the API
/// then streams headerless 24 kHz 16-bit mono LE PCM.
#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    response_format: &'static str,
    speed: f32,
}

/// Synthesizes `text` and returns the raw PCM bytes.
///
/// # Errors
///
/// Returns a provider error for transport failures and non-success statuses
/// (401/403 → `Auth`, 429 → `RateLimit`), and a typed `Truncated` error when
/// the streamed PCM ends mid-sample. Transient failures (network, 429) are
/// retried with jittered backoff, honoring the upstream `Retry-After`.
pub async fn synthesize(
    config: &OpenAiTtsConfig,
    api_key: &str,
    base_url: &str,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, PluginError> {
    let client = http_client()?;
    let url = format!("{}/audio/speech", base_url.trim_end_matches('/'));
    let body = SpeechRequest {
        model: &config.model,
        input: text,
        voice,
        response_format: "pcm",
        speed: config.speed,
    };

    let mut attempt: u32 = 0;
    loop {
        let sent = client
            .post(&url)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;

        let err = match sent {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let audio = read_body_bounded(response, MAX_PCM_BYTES).await?;
                    if audio.is_empty() {
                        return Err(PluginError::provider(
                            "speech API returned an empty audio response",
                        ));
                    }
                    pcm::validate_pcm(&audio)?;
                    return Ok(wav::wrap_pcm(&audio, config.sample_rate)?);
                }
                let retry_after = retry_after_secs(&response);
                let body = read_body_bounded(response, MAX_ERROR_BODY_BYTES)
                    .await
                    .map_err(|e| {
                        PluginError::provider(format!("failed to read error response: {e}"))
                    })?;
                UpstreamError::Http {
                    status: status.as_u16(),
                    body,
                    retry_after,
                }
            }
            Err(e) => UpstreamError::Network(format!("speech request failed: {e}")),
        };

        let next = attempt.saturating_add(1);
        if !err.is_retryable() || next >= MAX_ATTEMPTS {
            return Err(err.into_plugin_error());
        }
        let delay = match &err {
            UpstreamError::Http {
                retry_after: Some(secs),
                ..
            } => Duration::from_secs(*secs),
            _ => backoff_delay(attempt),
        }
        .min(MAX_DELAY);
        tracing::warn!(
            component = "ene-plugin-openai-tts",
            attempt = next,
            delay_ms = delay.as_millis() as u64,
            error = %err.into_plugin_error(),
            "retryable upstream failure; backing off"
        );
        tokio::time::sleep(delay).await;
        attempt = next;
    }
}

fn http_client() -> Result<&'static reqwest::Client, PluginError> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| PluginError::provider(format!("failed to build HTTP client: {e}")))?;
    // A racing task may have initialized first; either client is equivalent.
    drop(HTTP_CLIENT.set(client));
    HTTP_CLIENT
        .get()
        .ok_or_else(|| PluginError::provider("HTTP client initialization failed"))
}

/// Maps a non-success status to a typed provider error, with the upstream
/// body truncated to a snippet. The API key never appears here: only the
/// status and response body are echoed.
fn map_http_error(status: StatusCode, body: &[u8]) -> PluginError {
    let snippet: String = String::from_utf8_lossy(body).chars().take(280).collect();
    match status.as_u16() {
        401 | 403 => PluginError::provider_typed(
            ProviderErrorKind::Auth,
            format!("authentication failed: {snippet}"),
        ),
        429 => PluginError::provider_typed(
            ProviderErrorKind::RateLimit,
            format!("rate limited: {snippet}"),
        ),
        _ => PluginError::provider(format!("HTTP {}: {snippet}", status.as_u16())),
    }
}

/// An upstream failure before mapping to [`PluginError`]. Transport failures
/// and HTTP 429 are retryable; everything else is terminal.
enum UpstreamError {
    /// Transport-level failure (DNS, connect, timeout, mid-read).
    Network(String),
    /// Non-success HTTP status with the (possibly truncated) response body.
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body bytes.
        body: Vec<u8>,
        /// `Retry-After` header value, if present and parseable.
        retry_after: Option<u64>,
    },
}

impl UpstreamError {
    /// Whether the retry budget should spend an attempt on this error.
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Http { status: 429, .. })
    }

    /// Maps to a typed [`PluginError`] (see [`map_http_error`]).
    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Network(message) => PluginError::provider(message),
            Self::Http { status, body, .. } => map_http_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                &body,
            ),
        }
    }
}

/// Parses a `Retry-After` response header (seconds) if present and valid.
fn retry_after_secs(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// Jittered exponential backoff for retry attempt `retry_index` (0-indexed).
fn backoff_delay(retry_index: u32) -> Duration {
    let base_ms = BASE_DELAY.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(2u64.saturating_pow(retry_index));
    let capped = exp_ms.min(MAX_DELAY.as_millis() as u64);
    let jittered = if capped == 0 {
        0
    } else {
        rand::random_range(0..=capped)
    };
    Duration::from_millis(jittered)
}

/// Reads a response body into memory, rejecting payloads over `limit`
/// (checked against `Content-Length` upfront and against the accumulated
/// stream while reading, so a lying or chunked upstream cannot OOM the
/// plugin).
async fn read_body_bounded(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, PluginError> {
    if let Some(length) = response.content_length()
        && length > limit as u64
    {
        return Err(PluginError::provider(format!(
            "speech response too large: {length} bytes exceeds the {limit}-byte limit"
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| PluginError::provider(format!("failed to read speech response: {e}")))?;
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err(PluginError::provider(format!(
                "speech response exceeds the {limit}-byte limit"
            )));
        }
    }
    Ok(body)
}
