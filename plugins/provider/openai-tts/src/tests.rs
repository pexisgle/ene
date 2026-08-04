//! Plugin tests: synthesis against an in-process fake Speech API, error
//! paths, and capability/schema coverage.

#![expect(
    clippy::await_holding_lock,
    clippy::expect_used,
    reason = "tests serialize the process-wide PLUGIN_CONFIG across synthesis calls \
              and use expect for concise assertions"
)]

use std::sync::{Mutex as StdMutex, PoisonError};
use std::time::Duration;

use ene_plugin::{ConfigurablePlugin as _, PluginError, ProviderErrorKind, TtsPlugin as _};
use serde_json::{Value, json};

use crate::client::MAX_PCM_BYTES;
use crate::config::{
    DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_VOICE, SUPPORTED_VOICES, resolve_api_key,
    resolve_base_url,
};
use crate::mock_server::{MockResponse, MockSpeechServer};
use crate::plugin::{OpenAiTtsPlugin, PLUGIN_CONFIG};

const KIND: &str = "openai_tts";
const TEST_KEY: &str = "sk-test-123";

/// Serializes tests that read or write the process-wide `PLUGIN_CONFIG`
/// static (every synthesize call reads it).
static TEST_SERIAL: StdMutex<()> = StdMutex::new(());

/// Serializes tests that mutate the process environment.
static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

fn test_plugin() -> OpenAiTtsPlugin {
    OpenAiTtsPlugin
}

fn config_json(base_url: &str) -> Value {
    json!({
        "api_key": TEST_KEY,
        "base_url": base_url,
        "model": "tts-1-hd",
        "voice": "nova",
        "speed": 1.25
    })
}

/// A small valid PCM payload.
fn pcm_fixture() -> Vec<u8> {
    let samples = [0i16, 16_384, -16_384, 32_767, -32_768];
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn request_header<'a>(
    request: &'a crate::mock_server::RecordedRequest,
    name: &str,
) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value.as_str())
}

/// Asserts the returned bytes are a WAV wrapping exactly `expected_pcm` at
/// the Speech API's fixed 24 kHz sample rate.
fn assert_wav_payload(wav: &[u8], expected_pcm: &[u8]) {
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(
        u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
        24_000
    );
    assert_eq!(&wav[44..], expected_pcm);
}

fn mock_url(mock: &MockSpeechServer) -> String {
    format!("{}/v1", mock.url)
}

#[tokio::test]
async fn synthesis_returns_wav_and_sends_expected_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm.clone()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "こんにちは".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_wav_payload(&audio, &pcm);

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/audio/speech");
    assert_eq!(
        request_header(request, "authorization"),
        Some(format!("Bearer {TEST_KEY}").as_str())
    );
    assert!(
        request_header(request, "content-type")
            .is_some_and(|value| value.starts_with("application/json"))
    );
    let body: Value = serde_json::from_str(&request.body).expect("request body is JSON");
    assert_eq!(body["model"], "tts-1-hd");
    assert_eq!(body["input"], "こんにちは");
    assert_eq!(body["voice"], "nova");
    assert_eq!(body["response_format"], "pcm");
    assert!((body["speed"].as_f64().expect("speed is a number") - 1.25).abs() < 1e-6);
}

#[tokio::test]
async fn per_request_voice_overrides_configured_voice() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            "shimmer".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    assert_eq!(body["voice"], "shimmer");
}

#[tokio::test]
async fn omitted_settings_fall_back_to_api_defaults() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({ "api_key": TEST_KEY, "base_url": mock_url(&mock) }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    assert_eq!(body["model"], DEFAULT_MODEL);
    assert_eq!(body["voice"], DEFAULT_VOICE);
    assert!((body["speed"].as_f64().expect("speed is a number") - 1.0).abs() < 1e-6);
}

#[tokio::test]
async fn streamed_chunked_response_is_accumulated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::streamed(
        pcm.clone(),
        4,
        Duration::from_millis(5),
    ));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "streaming".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("streamed synthesis succeeds");
    assert_wav_payload(&audio, &pcm);
}

#[tokio::test]
async fn auth_errors_map_to_typed_auth() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    for status in [401, 403] {
        let mock = MockSpeechServer::spawn().expect("mock server");
        mock.push(MockResponse::with_status(
            status,
            b"{\"error\":\"bad key\"}".to_vec(),
        ));

        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(&mock_url(&mock)),
                "hello".to_string(),
                String::new(),
                "wav".to_string(),
            )
            .await
            .expect_err("auth failure surfaces");
        assert_eq!(err.provider_error_kind(), Some(ProviderErrorKind::Auth));
        assert!(!err.to_string().contains(TEST_KEY));
    }
}

#[tokio::test]
async fn rate_limit_maps_to_typed_rate_limit() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::with_status(
        429,
        b"{\"error\":\"slow down\"}".to_vec(),
    ));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("rate limit surfaces");
    assert_eq!(
        err.provider_error_kind(),
        Some(ProviderErrorKind::RateLimit)
    );
}

#[tokio::test]
async fn server_error_includes_status_and_snippet() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::with_status(
        500,
        b"{\"error\":\"upstream exploded\"}".to_vec(),
    ));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("server error surfaces");
    assert!(err.provider_error_kind().is_none());
    let message = err.to_string();
    assert!(message.contains("HTTP 500"));
    assert!(message.contains("upstream exploded"));
}

#[tokio::test]
async fn truncated_pcm_maps_to_typed_truncated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(vec![0, 0, 1]));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("odd-length PCM surfaces");
    assert_eq!(
        err.provider_error_kind(),
        Some(ProviderErrorKind::Truncated)
    );
}

#[tokio::test]
async fn oversized_content_length_is_rejected_before_reading() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()).with_declared_length(MAX_PCM_BYTES + 1));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized payload rejected");
    assert!(err.to_string().contains("too large"));
}

#[tokio::test]
async fn oversized_streamed_body_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    mock.push(MockResponse::streamed(
        vec![0u8; MAX_PCM_BYTES + 2],
        8 * 1024,
        Duration::ZERO,
    ));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized stream rejected");
    assert!(err.to_string().contains("exceeds"));
}

#[tokio::test]
async fn rejects_unsupported_formats() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    for format in ["mp3", "pcm"] {
        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(&mock_url(&mock)),
                "hello".to_string(),
                String::new(),
                format.to_string(),
            )
            .await
            .expect_err("unsupported format rejected");
        assert!(err.to_string().contains("wav"), "format {format}: {err}");
    }
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn rejects_empty_text() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "   ".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("empty text rejected");
    assert!(err.to_string().contains("empty"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn rejects_unknown_provider_kind() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            "voicevox",
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unknown kind rejected");
    assert!(matches!(err, PluginError::NotSupported(_)));
}

#[tokio::test]
async fn missing_api_key_fails_without_hitting_the_network() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("OPENAI_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
    let mock = MockSpeechServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({ "base_url": mock_url(&mock) }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("missing key rejected");
    assert!(err.to_string().contains("no API key"));
    assert!(mock.requests.lock().expect("request log").is_empty());
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("OPENAI_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        },
    }
}

#[tokio::test]
async fn out_of_range_speed_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "speed": 4.5
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("out-of-range speed rejected");
    assert!(err.to_string().contains("speed"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[test]
fn capabilities_declare_kind_voices_formats_and_concurrency() {
    let capabilities = test_plugin().tts_capabilities();
    assert_eq!(capabilities.len(), 1);
    let spec = &capabilities[0];
    assert_eq!(spec.kind, KIND);
    assert_eq!(spec.voices, SUPPORTED_VOICES);
    assert_eq!(spec.formats, vec!["wav"]);
    assert_eq!(spec.concurrency.max_in_flight, 8);
    assert_eq!(spec.concurrency.queue_depth, 16);
}

#[test]
fn config_schema_marks_api_key_secret_and_constrains_settings() {
    let schema = test_plugin().config_schema().expect("schema present");
    assert_eq!(schema["properties"]["api_key"]["x-ene-secret"], true);
    assert_eq!(schema["properties"]["model"]["enum"][0], "tts-1");
    assert_eq!(
        schema["properties"]["voice"]["enum"],
        json!(SUPPORTED_VOICES)
    );
    assert_eq!(schema["properties"]["speed"]["minimum"], 0.25);
    assert_eq!(schema["properties"]["speed"]["maximum"], 4.0);
}

#[test]
fn set_config_key_is_used_by_synthesis_resolution() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    test_plugin().set_config(&json!({"api_key": "sk-via-set-config"}));

    let host = PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    let resolved = resolve_api_key(host.as_ref(), &json!({})).expect("key resolves");
    assert_eq!(resolved, "sk-via-set-config");
    drop(host);

    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

#[test]
fn resolves_plain_string_keys() {
    let config = json!({"api_key": "sk-plain-789"});
    assert_eq!(
        resolve_api_key(None, &config).expect("plain key resolves"),
        "sk-plain-789"
    );
}

#[test]
fn resolves_inline_descriptor() {
    let config = json!({"api_key": {"source": "inline", "inline": "sk-inline-123"}});
    assert_eq!(
        resolve_api_key(None, &config).expect("inline key resolves"),
        "sk-inline-123"
    );
}

#[test]
fn resolves_env_descriptor() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::set_var("ENE_TEST_OPENAI_TTS_KEY", "sk-env-test-456");
    }
    let config = json!({"api_key": {"source": "env", "env": "ENE_TEST_OPENAI_TTS_KEY"}});
    assert_eq!(
        resolve_api_key(None, &config).expect("env key resolves"),
        "sk-env-test-456"
    );
    // SAFETY: test-only cleanup, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("ENE_TEST_OPENAI_TTS_KEY");
    }
}

#[test]
fn host_config_key_wins_over_request_config() {
    let host = json!({"api_key": "sk-host-wins"});
    let request = json!({"api_key": "sk-request-loses"});
    assert_eq!(
        resolve_api_key(Some(&host), &request).expect("host key wins"),
        "sk-host-wins"
    );
}

#[test]
fn process_env_is_the_last_fallback() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("OPENAI_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", "sk-auto-000");
    }
    assert_eq!(
        resolve_api_key(None, &json!({})).expect("env fallback resolves"),
        "sk-auto-000"
    );
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("OPENAI_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        },
    }
}

#[test]
fn missing_key_everywhere_is_an_error() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("OPENAI_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("OPENAI_API_KEY");
    }
    let err = resolve_api_key(None, &json!({})).expect_err("no key anywhere");
    assert!(err.to_string().contains("no API key"));
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("OPENAI_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        },
    }
}

#[test]
fn base_url_precedence_and_default() {
    assert_eq!(resolve_base_url(None, &json!({})), DEFAULT_BASE_URL);
    assert_eq!(
        resolve_base_url(None, &json!({"base_url": "https://request.example/v1"})),
        "https://request.example/v1"
    );
    assert_eq!(
        resolve_base_url(
            Some(&json!({"base_url": "https://host.example/v1"})),
            &json!({"base_url": "https://request.example/v1"})
        ),
        "https://host.example/v1"
    );
}
