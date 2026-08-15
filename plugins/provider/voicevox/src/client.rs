//! VOICEVOX-compatible HTTP client: the 2-step `audio_query` →
//! `synthesis` flow and the `/version` health probe.
//!
//! Both VOICEVOX and Aivis Speech serialize `AudioQuery` with camelCase
//! field names; Aivis adds `tempoDynamicsScale`. The query response is
//! round-tripped with unknown fields preserved, so a future engine extension
//! survives the plugin untouched.

use std::sync::OnceLock;
use std::time::Duration;

use ene_plugin::PluginError;
use futures::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::VoicevoxConfig;

/// Shared HTTP client with a conservative connect timeout; per-request
/// overall timeouts are applied at the call site (health probes need a much
/// shorter bound than synthesis).
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> Result<&'static reqwest::Client, PluginError> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| PluginError::provider(format!("failed to build HTTP client: {e}")))?;
    // A racing task may have initialized first; either client is equivalent.
    drop(HTTP_CLIENT.set(client));
    HTTP_CLIENT
        .get()
        .ok_or_else(|| PluginError::provider("HTTP client initialization failed"))
}

/// Per-request timeout for the two synthesis steps.
const SYNTHESIS_TIMEOUT: Duration = Duration::from_mins(1);
/// Per-probe timeout for `GET /version` health checks.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// Cap on any engine response body. 24 kHz s16 mono audio is ~2.9 MB per
/// minute, so 32 MiB covers very long utterances while bounding the memory
/// a misbehaving engine can make the plugin allocate.
///
/// The host rejects WAV payloads above `ene-plugin-host`'s `MAX_WAV_BYTES`
/// (32 MiB), so a larger response would fail at the host anyway.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Engine-reported synthesis parameters, round-tripped between the two API
/// steps with the configured scale overrides applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioQuery {
    /// Phoneme/accent structure computed by the engine. Opaque here: the
    /// plugin only carries it between `audio_query` and `synthesis`.
    pub accent_phrases: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intonation_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_scale: Option<f32>,
    /// Aivis Speech extension: only emitted when configured off-default,
    /// because VOICEVOX rejects unknown fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo_dynamics_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_sampling_rate: Option<u32>,
    /// Unknown engine fields (e.g. `prePhonemeLength`, `kana`) survive the
    /// round-trip untouched.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AudioQuery {
    /// Applies the configured scale and sampling overrides to a query
    /// returned by `audio_query`.
    ///
    /// The four standard scales are always overridden by the config. The
    /// Aivis extension fields are only overridden when explicitly
    /// configured: engine-returned values survive at config defaults, so an
    /// Aivis preset's `outputSamplingRate` / `tempoDynamicsScale` round-trip
    /// (VOICEVOX never returns them, so it still never sees them).
    #[must_use]
    pub fn with_overrides(mut self, config: &VoicevoxConfig) -> Self {
        self.speed_scale = Some(config.speed_scale);
        self.pitch_scale = Some(config.pitch_scale);
        self.intonation_scale = Some(config.intonation_scale);
        self.volume_scale = Some(config.volume_scale);
        if (config.tempo_dynamics_scale - 1.0).abs() > f32::EPSILON {
            self.tempo_dynamics_scale = Some(config.tempo_dynamics_scale);
        }
        if let Some(rate) = config.output_sampling_rate {
            self.output_sampling_rate = Some(rate);
        }
        self
    }
}

/// Whether an engine is currently serving `GET /version` at `server_url`.
pub async fn engine_reachable(config: &VoicevoxConfig) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let Ok(response) = client
        .get(format!(
            "{}/version",
            config.server_url.trim_end_matches('/')
        ))
        .timeout(HEALTH_TIMEOUT)
        .send()
        .await
    else {
        return false;
    };
    response.status().is_success()
}

/// One speaker (character) as reported by the engine's `/speakers` endpoint,
/// with its selectable styles.
#[derive(Debug, Clone, Deserialize)]
pub struct Speaker {
    /// Character name, used as the option group label.
    pub name: String,
    /// Voice styles belonging to this character.
    pub styles: Vec<Style>,
}

/// One selectable style within a [`Speaker`].
#[derive(Debug, Clone, Deserialize)]
pub struct Style {
    /// Style name (e.g. "ノーマル").
    pub name: String,
    /// Style id, written into `speaker_id`.
    pub id: u64,
}

/// Fetches the engine's speaker list for the settings UI.
///
/// Uses the same short health-probe timeout as [`engine_reachable`] so a
/// down engine answers quickly with an error instead of hanging the settings
/// page. Never spawns a managed engine — listing speakers is a read-only
/// query against whatever is already running.
///
/// # Errors
///
/// Returns a provider error when the engine is unreachable or the response
/// is not the documented `/speakers` shape.
pub async fn fetch_speakers(config: &VoicevoxConfig) -> Result<Vec<Speaker>, PluginError> {
    let client = http_client()?;
    let url = format!("{}/speakers", config.server_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .timeout(HEALTH_TIMEOUT)
        .send()
        .await
        .map_err(|e| PluginError::provider(format!("GET {url} failed: {e}")))?;
    parse_json_response(response, "/speakers").await
}

/// Runs the 2-step synthesis flow and returns the WAV bytes.
///
/// # Errors
///
/// Returns a provider error when either HTTP step fails (network error,
/// non-success status, or an unparseable `AudioQuery` response).
pub async fn synthesize(
    config: &VoicevoxConfig,
    text: &str,
    speaker: u64,
) -> Result<Vec<u8>, PluginError> {
    let client = http_client()?;
    let base = config.server_url.trim_end_matches('/');

    let query_url = reqwest::Url::parse_with_params(
        &format!("{base}/audio_query"),
        &[("text", text), ("speaker", &speaker.to_string())],
    )
    .map_err(|e| PluginError::provider(format!("invalid audio_query URL: {e}")))?;
    let query_response = client
        .post(query_url)
        .timeout(SYNTHESIS_TIMEOUT)
        .send()
        .await
        .map_err(|e| PluginError::provider(format!("audio_query request failed: {e}")))?;
    let query: AudioQuery = parse_json_response(query_response, "/audio_query").await?;
    let query = query.with_overrides(config);

    let synthesis_url = reqwest::Url::parse_with_params(
        &format!("{base}/synthesis"),
        &[("speaker", &speaker.to_string())],
    )
    .map_err(|e| PluginError::provider(format!("invalid synthesis URL: {e}")))?;
    let body = serde_json::to_vec(&query)
        .map_err(|e| PluginError::provider(format!("failed to serialize AudioQuery: {e}")))?;
    let synthesis_response = client
        .post(synthesis_url)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(SYNTHESIS_TIMEOUT)
        .send()
        .await
        .map_err(|e| PluginError::provider(format!("synthesis request failed: {e}")))?;
    let (status, bytes) = read_response_bounded(synthesis_response, "/synthesis").await?;
    if !status.is_success() {
        return Err(non_success(status, "/synthesis", &bytes));
    }
    Ok(bytes)
}

async fn parse_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    endpoint: &str,
) -> Result<T, PluginError> {
    let (status, bytes) = read_response_bounded(response, endpoint).await?;
    if !status.is_success() {
        return Err(non_success(status, endpoint, &bytes));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| PluginError::provider(format!("invalid JSON from {endpoint}: {e}")))
}

/// Reads a response body into memory, rejecting payloads over
/// [`MAX_RESPONSE_BYTES`] (checked against `Content-Length` upfront and
/// against the accumulated stream while reading, so a lying or chunked
/// engine cannot OOM the plugin).
async fn read_response_bounded(
    response: reqwest::Response,
    endpoint: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), PluginError> {
    let status = response.status();
    if let Some(length) = response.content_length()
        && length > MAX_RESPONSE_BYTES as u64
    {
        return Err(PluginError::provider(format!(
            "{endpoint} response too large: {length} bytes exceeds the \
             {MAX_RESPONSE_BYTES}-byte limit"
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            PluginError::provider(format!("failed to read {endpoint} response: {e}"))
        })?;
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(PluginError::provider(format!(
                "{endpoint} response exceeds the {MAX_RESPONSE_BYTES}-byte limit"
            )));
        }
    }
    Ok((status, body))
}

fn non_success(status: StatusCode, endpoint: &str, body: &[u8]) -> PluginError {
    let detail = String::from_utf8_lossy(body);
    let detail = detail.chars().take(300).collect::<String>();
    PluginError::provider(format!(
        "voicevox {endpoint} failed with HTTP {status}: {detail}"
    ))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;
    use crate::mock_engine::EXAMPLE_AUDIO_QUERY;
    use serde_json::json;

    fn test_config() -> VoicevoxConfig {
        VoicevoxConfig {
            speed_scale: 1.5,
            pitch_scale: 0.05,
            intonation_scale: 0.8,
            volume_scale: 0.9,
            tempo_dynamics_scale: 1.2,
            output_sampling_rate: Some(48_000),
            ..VoicevoxConfig::default()
        }
    }

    #[test]
    fn parses_engine_query_with_camel_case_fields() {
        let query: AudioQuery =
            serde_json::from_str(EXAMPLE_AUDIO_QUERY).expect("example query parses");
        assert_eq!(query.speed_scale, Some(1.2));
        assert_eq!(query.output_sampling_rate, Some(24_000));
        assert!(query.extra.contains_key("prePhonemeLength"));
        assert!(query.extra.contains_key("kana"));
    }

    #[test]
    fn applies_configured_overrides_and_preserves_unknown_fields() {
        let query: AudioQuery =
            serde_json::from_str(EXAMPLE_AUDIO_QUERY).expect("example query parses");
        let overridden = query.with_overrides(&test_config());
        assert_eq!(overridden.speed_scale, Some(1.5));
        assert_eq!(overridden.pitch_scale, Some(0.05));
        assert_eq!(overridden.intonation_scale, Some(0.8));
        assert_eq!(overridden.volume_scale, Some(0.9));
        assert_eq!(overridden.tempo_dynamics_scale, Some(1.2));
        assert_eq!(overridden.output_sampling_rate, Some(48_000));
        let serialized = serde_json::to_value(&overridden).expect("serializes");
        assert_eq!(serialized["accentPhrases"][0]["moras"][0]["vowel"], "o");
        assert!((serialized["prePhonemeLength"].as_f64().expect("number") - 0.1).abs() < 1e-6);
        assert_eq!(serialized["kana"], "コ");
        assert!((serialized["tempoDynamicsScale"].as_f64().expect("number") - 1.2).abs() < 1e-6);
    }

    #[test]
    fn preserves_engine_returned_aivis_extensions_at_defaults() {
        let query: AudioQuery =
            serde_json::from_str(EXAMPLE_AUDIO_QUERY).expect("example query parses");
        let config = VoicevoxConfig {
            tempo_dynamics_scale: 1.0,
            output_sampling_rate: None,
            ..VoicevoxConfig::default()
        };
        let serialized = serde_json::to_value(query.with_overrides(&config)).expect("serializes");
        // The engine returned `outputSamplingRate`; with no config override
        // it must round-trip instead of being erased.
        assert_eq!(serialized["outputSamplingRate"], 24_000);
        // The example engine never returns `tempoDynamicsScale` (it is a
        // VOICEVOX-shaped response), so it stays absent.
        assert!(serialized.get("tempoDynamicsScale").is_none());
        assert!((serialized["speedScale"].as_f64().expect("number") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn config_overrides_engine_returned_aivis_extensions() {
        let query: AudioQuery = serde_json::from_value(json!({
            "accentPhrases": [],
            "outputSamplingRate": 48000,
            "tempoDynamicsScale": 1.5
        }))
        .expect("aivis query parses");
        let config = VoicevoxConfig {
            tempo_dynamics_scale: 0.8,
            output_sampling_rate: Some(24_000),
            ..VoicevoxConfig::default()
        };
        let serialized = serde_json::to_value(query.with_overrides(&config)).expect("serializes");
        assert_eq!(serialized["outputSamplingRate"], 24_000);
        let tempo = serialized["tempoDynamicsScale"]
            .as_f64()
            .expect("tempo is a number");
        assert!((tempo - 0.8).abs() < 1e-6, "tempo={tempo}");
    }

    #[test]
    fn missing_accent_phrases_is_an_error() {
        let err: Result<AudioQuery, _> = serde_json::from_value(json!({"speedScale": 1.0}));
        assert!(err.is_err());
    }
}
