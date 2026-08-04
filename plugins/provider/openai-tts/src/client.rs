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
/// Sample rate of the Speech API's `pcm` format (fixed by the API).
const SAMPLE_RATE: u32 = 24_000;
/// Cap on the raw PCM body. 24 kHz s16 mono is ~2.9 MB per minute, so
/// 32 MiB covers very long utterances while bounding the memory a
/// misbehaving upstream can make the plugin allocate. The 44-byte WAV
/// header keeps the wrapped payload at or under the host adapter's
/// `MAX_WAV_BYTES` cap.
pub(crate) const MAX_PCM_BYTES: usize = 32 * 1024 * 1024 - 44;
/// Cap on error-response bodies; only a snippet is surfaced anyway.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

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
/// the streamed PCM ends mid-sample.
pub async fn synthesize(
    config: &OpenAiTtsConfig,
    api_key: &str,
    base_url: &str,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, PluginError> {
    let client = http_client()?;
    let url = format!("{}/audio/speech", base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(api_key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&SpeechRequest {
            model: &config.model,
            input: text,
            voice,
            response_format: "pcm",
            speed: config.speed,
        })
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| PluginError::provider(format!("speech request failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = read_body_bounded(response, MAX_ERROR_BODY_BYTES).await?;
        return Err(map_http_error(status, &body));
    }
    let audio = read_body_bounded(response, MAX_PCM_BYTES).await?;
    // Decoding doubles as stream-integrity validation (odd trailing byte =
    // truncated stream); the raw bytes are then wrapped in WAV, which is
    // the container the host-side adapter decodes.
    drop(pcm::samples_from_bytes(&audio)?);
    Ok(wav::wrap_pcm(&audio, SAMPLE_RATE))
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
