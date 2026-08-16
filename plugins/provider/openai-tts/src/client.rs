//! `OpenAI Speech API` client: request building, retries, and typed error
//! mapping over the host-mediated network broker.

use std::time::Duration;

use ene_plugin::{PluginError, ProviderErrorKind};
use ene_plugin_broker::HttpMethod;
use serde::Serialize;

use crate::broker::broker;
use crate::config::OpenAiTtsConfig;
use crate::pcm;
use crate::wav;

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

/// Synthesizes `text` and returns WAV bytes.
///
/// # Errors
///
/// Returns a provider error for transport failures and non-success statuses
/// (401/403 → `Auth`, 429 → `RateLimit`), and a typed `Truncated` error when
/// the returned PCM ends mid-sample. Transient failures (network, 429) are
/// retried with jittered backoff, honoring the upstream `Retry-After`. The
/// host injects the `"api_key"` credential as `Authorization: Bearer`.
pub async fn synthesize(
    config: &OpenAiTtsConfig,
    base_url: &str,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, PluginError> {
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
        let payload = serde_json::to_vec(&body)
            .map_err(|e| PluginError::provider(format!("failed to serialize request: {e}")))?;
        let sent = broker()
            .fetch(
                HttpMethod::Post,
                &url,
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some("api_key"),
                None,
                Some(payload),
            )
            .await;

        let err = match sent {
            Ok(response) => {
                let status = response.status;
                if (200..300).contains(&status) {
                    let audio = response.body;
                    if audio.len() > MAX_PCM_BYTES {
                        return Err(PluginError::provider(format!(
                            "speech response exceeds the {MAX_PCM_BYTES}-byte limit"
                        )));
                    }
                    if audio.is_empty() {
                        return Err(PluginError::provider(
                            "speech API returned an empty audio response",
                        ));
                    }
                    pcm::validate_pcm(&audio)?;
                    return wav::wrap_pcm(&audio, config.sample_rate);
                }
                let retry_after = retry_after_secs(&response.headers);
                let body: Vec<u8> = response
                    .body
                    .into_iter()
                    .take(MAX_ERROR_BODY_BYTES)
                    .collect();
                UpstreamError::Http {
                    status,
                    body,
                    retry_after,
                }
            }
            Err(e) => UpstreamError::Network(format!("broker request failed: {e}")),
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

/// Maps a non-success status to a typed provider error, with the upstream
/// body truncated to a snippet. The API key never appears here: only the
/// status and response body are echoed.
fn map_http_error(status: u16, body: &[u8]) -> PluginError {
    let snippet: String = String::from_utf8_lossy(body).chars().take(280).collect();
    match status {
        401 | 403 => PluginError::provider_typed(
            ProviderErrorKind::Auth,
            format!("authentication failed: {snippet}"),
        ),
        429 => PluginError::provider_typed(
            ProviderErrorKind::RateLimit,
            format!("rate limited: {snippet}"),
        ),
        _ => PluginError::provider(format!("HTTP {status}: {snippet}")),
    }
}

/// An upstream failure before mapping to [`PluginError`]. Transport failures
/// and HTTP 429 are retryable; everything else is terminal.
enum UpstreamError {
    /// Transport-level failure (DNS, connect, timeout, mid-read).
    Network(String),
    /// Non-success HTTP status with the (possibly truncated) response body.
    Http {
        status: u16,
        body: Vec<u8>,
        /// `Retry-After` header value, if present and parseable.
        retry_after: Option<u64>,
    },
}

impl UpstreamError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Http { status: 429, .. })
    }

    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Network(message) => PluginError::provider(message),
            Self::Http { status, body, .. } => map_http_error(status, &body),
        }
    }
}

fn retry_after_secs(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
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
