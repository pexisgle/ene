//! `ElevenLabs` REST client: request building, retries, and typed error
//! mapping over the host-mediated network broker.

use std::time::Duration;

use ene_plugin::{PluginError, ProviderErrorKind};
use ene_plugin_broker::HttpMethod;
use serde::Serialize;

use crate::broker::broker;
use crate::config::{ElevenLabsConfig, VoiceSettings, resolve_base_url};
use crate::pcm;
use crate::wav;

/// Retry budget for transient upstream failures (transient statuses /
/// network); the host TTS consumer never retries, so the plugin absorbs
/// them like the openai plugin does.
pub(crate) const MAX_ATTEMPTS: u32 = 3;
/// Base backoff for retry attempts, doubled per attempt and jittered.
pub(crate) const BASE_DELAY: Duration = Duration::from_millis(500);
/// Upper bound on any single backoff (including a server `Retry-After`).
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(30);
/// Cap on the raw PCM body. 24 kHz s16 mono is ~2.9 MB per minute, so
/// 32 MiB covers very long utterances while bounding the memory a
/// misbehaving upstream can make the plugin allocate. The 44-byte WAV
/// header keeps the wrapped payload at or under the host adapter's
/// `MAX_WAV_BYTES` cap.
pub(crate) const MAX_PCM_BYTES: usize = 32 * 1024 * 1024 - 44;
/// Cap on error-response bodies; only a snippet is surfaced anyway.
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// `/stream` request body. `output_format` is a query parameter; the API
/// then streams headerless 16-bit mono LE PCM at the requested rate.
#[derive(Serialize)]
struct SpeechRequest<'a> {
    text: &'a str,
    model_id: &'a str,
    voice_settings: VoiceSettings,
}

/// Synthesizes `text` over the REST endpoint and returns WAV bytes.
///
/// # Errors
///
/// Returns a provider error for transport failures and non-success statuses
/// (401/403 → `Auth`, 429 → `RateLimit`), and a typed `Truncated` error when
/// the returned PCM ends mid-sample. Transient failures (network, 408/429,
/// 5xx) are retried with jittered backoff, honoring the upstream
/// `Retry-After`. The host injects the `"api_key"` credential as
/// `xi-api-key`.
pub async fn synthesize_rest(
    config: &ElevenLabsConfig,
    base_url: &str,
    text: &str,
    voice_id: &str,
) -> Result<Vec<u8>, PluginError> {
    let url = format!(
        "{}/text-to-speech/{voice_id}/stream?output_format=pcm_{}",
        base_url.trim_end_matches('/'),
        config.sample_rate
    );
    let body = SpeechRequest {
        text,
        model_id: &config.model_id,
        voice_settings: config.voice_settings,
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
                Some("xi-api-key"),
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
                            "elevenlabs response exceeds the {MAX_PCM_BYTES}-byte limit"
                        )));
                    }
                    if audio.is_empty() {
                        return Err(PluginError::provider(
                            "elevenlabs returned an empty audio response",
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
        let delay = retry_delay(err.retry_after(), attempt);
        tracing::warn!(
            component = "ene-plugin-elevenlabs",
            attempt = next,
            delay_ms = delay.as_millis() as u64,
            error = %err.into_plugin_error(),
            "retryable upstream failure; backing off"
        );
        tokio::time::sleep(delay).await;
        attempt = next;
    }
}

/// Response shape of the `/voices` endpoint (only the fields the settings
/// UI needs; unknown fields are ignored).
#[derive(serde::Deserialize)]
struct VoicesResponse {
    voices: Vec<VoiceSummary>,
}

#[derive(serde::Deserialize)]
struct VoiceSummary {
    voice_id: String,
    name: String,
}

/// Fetches the account's voice list for the settings UI.
///
/// Uses the same host-mediated broker as synthesis (the `api_key`
/// credential is injected as `xi-api-key`). A missing credential, network
/// failure, or non-2xx response yields `Ok(vec![])` — the caller treats an
/// unavailable voice list as "keep the free-form field", never as a
/// configuration error. The response is bounded to [`MAX_ERROR_BODY_BYTES`];
/// a real voice list is a few KB, so the cap is generous yet safe.
pub async fn fetch_voices(
    config: &ElevenLabsConfig,
) -> Result<Vec<ene_plugin::ConfigOption>, PluginError> {
    let base_url = resolve_base_url(
        None,
        &serde_json::json!({
            "base_url": config.base_url
        }),
    );
    let url = format!("{}/voices", base_url.trim_end_matches('/'));
    let response = broker()
        .fetch(
            HttpMethod::Get,
            &url,
            Vec::new(),
            Some("api_key"),
            Some("xi-api-key"),
            None,
        )
        .await;
    let response = match response {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(
                component = "ene-plugin-elevenlabs",
                error = %e,
                "voice list unavailable; keeping free-form voice field"
            );
            return Ok(Vec::new());
        }
    };
    if !(200..300).contains(&response.status) {
        tracing::warn!(
            component = "ene-plugin-elevenlabs",
            status = response.status,
            "voice list unavailable; keeping free-form voice field"
        );
        return Ok(Vec::new());
    }
    let body: Vec<u8> = response
        .body
        .into_iter()
        .take(MAX_ERROR_BODY_BYTES)
        .collect();
    let parsed: VoicesResponse = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(
                component = "ene-plugin-elevenlabs",
                error = %e,
                "voice list unparseable; keeping free-form voice field"
            );
            return Ok(Vec::new());
        }
    };
    Ok(parsed
        .voices
        .into_iter()
        .map(|voice| ene_plugin::ConfigOption {
            value: serde_json::json!(voice.voice_id),
            label: voice.name,
            group: None,
        })
        .collect())
}

/// Maps a non-success status to a typed provider error, with the upstream
/// body truncated to a snippet.
fn map_http_error(status: u16, body: &[u8]) -> PluginError {
    let snippet =
        parse_error_message(body).unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let snippet: String = snippet.chars().take(280).collect();
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

/// Extracts a readable message from an `ElevenLabs` error body. The API wraps
/// errors in `detail` (a string or an object with `message`); some endpoints
/// use `error` with the same two shapes.
fn parse_error_message(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    for key in ["detail", "error"] {
        match value.get(key) {
            Some(serde_json::Value::String(message)) if !message.trim().is_empty() => {
                return Some(message.clone());
            }
            Some(serde_json::Value::Object(obj)) => {
                if let Some(message) = obj
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .filter(|message| !message.trim().is_empty())
                {
                    return Some(message.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// An upstream failure before mapping to [`PluginError`]. Transport failures
/// and transient HTTP statuses are retryable; everything else is terminal.
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
        match self {
            Self::Network(_) => true,
            Self::Http { status, .. } => is_transient_status(*status),
        }
    }

    fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            Self::Network(_) => None,
        }
    }

    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Network(message) => PluginError::provider(message),
            Self::Http { status, body, .. } => map_http_error(status, &body),
        }
    }
}

/// Client timeout, rate limit, and server-side failures.
#[must_use]
pub(crate) fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Parses a `Retry-After` response header, in either of the RFC 9110 forms:
/// integer seconds or an HTTP-date.
pub(crate) fn retry_after_secs(headers: &[(String, String)]) -> Option<u64> {
    let value = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
        .map(|(_, value)| value.as_str())?
        .trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    let when = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let secs = when
        .with_timezone(&chrono::Utc)
        .signed_duration_since(chrono::Utc::now())
        .num_seconds()
        .max(0);
    u64::try_from(secs).ok()
}

/// Sleep duration for retry `retry_index` (0-indexed): the upstream
/// `Retry-After` wins, otherwise a jittered exponential backoff.
#[must_use]
pub(crate) fn retry_delay(retry_after: Option<u64>, retry_index: u32) -> Duration {
    match retry_after {
        Some(secs) => Duration::from_secs(secs),
        None => backoff_delay(retry_index),
    }
    .min(MAX_DELAY)
}

#[must_use]
pub(crate) fn backoff_delay(retry_index: u32) -> Duration {
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
