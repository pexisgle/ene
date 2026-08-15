//! Plugin tests: synthesis against an in-process broker-frame fake, error
//! paths, and capability/schema coverage.

#![expect(
    clippy::await_holding_lock,
    clippy::expect_used,
    reason = "tests serialize the process-wide PLUGIN_CONFIG across synthesis calls \
              and use expect for concise assertions"
)]

use std::sync::{Mutex as StdMutex, PoisonError};

use ene_plugin::{ConfigurablePlugin as _, PluginError, ProviderErrorKind, TtsPlugin as _};
use ene_plugin_proto::SandboxConfigData;
use serde_json::{Value, json};

use crate::broker::broker;
use crate::client::MAX_PCM_BYTES;
use crate::config::{
    DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_VOICE, SUPPORTED_VOICES, resolve_base_url,
};
use crate::mock_server::{MockResponse, MockSpeechServer, RecordedRequest};
use crate::plugin::{OpenAiTtsPlugin, PLUGIN_CONFIG};

const KIND: &str = "openai_tts";

/// Serializes tests that read or write the process-wide `PLUGIN_CONFIG`
/// static or the shared broker (every synthesize call touches both).
static TEST_SERIAL: StdMutex<()> = StdMutex::new(());

/// Serializes tests that mutate the process environment.
static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

fn test_plugin() -> OpenAiTtsPlugin {
    OpenAiTtsPlugin
}

/// Points the shared broker at `mock`'s socket. The static broker caches
/// its session, so each test resets it first; `TEST_SERIAL` keeps the
/// shared static from racing across tests.
async fn configure_broker(mock: &MockSpeechServer) {
    broker().reset().await;
    broker().configure(&SandboxConfigData {
        broker_socket: Some(mock.socket_path().to_string_lossy().into_owned()),
        db_auth_token: Some("tok".to_string()),
        ..SandboxConfigData::default()
    });
}

fn config_json(base_url: &str) -> Value {
    json!({
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

fn request_header<'a>(request: &'a RecordedRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
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

#[tokio::test]
async fn synthesis_returns_wav_and_sends_expected_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm.clone()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
    assert_eq!(request.url, "https://api.openai.com/v1/audio/speech");
    // The plugin names the host-owned credential instead of sending a key.
    assert_eq!(request.credential.as_deref(), Some("api_key"));
    assert_eq!(request.credential_header, None);
    assert!(
        request_header(request, "content-type")
            .is_some_and(|value| value.starts_with("application/json"))
    );
    assert!(
        request_header(request, "authorization").is_none(),
        "the plugin must not send authorization headers itself"
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
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({"base_url": MockSpeechServer::url()}),
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
async fn auth_errors_map_to_typed_auth() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    for status in [401, 403] {
        let mock = MockSpeechServer::spawn().expect("mock server");
        configure_broker(&mock).await;
        mock.push(MockResponse::with_status(
            status,
            br#"{"error":"bad key"}"#.to_vec(),
        ));

        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(MockSpeechServer::url()),
                "hello".to_string(),
                String::new(),
                "wav".to_string(),
            )
            .await
            .expect_err("auth failure surfaces");
        assert_eq!(err.provider_error_kind(), Some(ProviderErrorKind::Auth));
    }
}

#[tokio::test]
async fn rate_limit_maps_to_typed_rate_limit() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for _ in 0..3 {
        mock.push(
            MockResponse::with_status(429, br#"{"error":"slow down"}"#.to_vec())
                .with_header("Retry-After", "0"),
        );
    }

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
    configure_broker(&mock).await;
    mock.push(MockResponse::with_status(
        500,
        br#"{"error":"upstream exploded"}"#.to_vec(),
    ));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(vec![0, 0, 1]));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
async fn oversized_audio_body_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(vec![0u8; MAX_PCM_BYTES + 2]));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized payload rejected");
    assert!(err.to_string().contains("exceeds"));
}

#[tokio::test]
async fn oversized_error_body_is_bounded() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::with_status(500, b"boom-".repeat(20_000)));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("server error surfaces");
    let message = err.to_string();
    assert!(message.contains("boom-"), "snippet prefix must survive");
    assert!(
        message.len() < 1_000,
        "error snippet must stay bounded, got {} chars",
        message.len()
    );
}

#[tokio::test]
async fn rejects_unsupported_formats() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for format in ["mp3", "pcm"] {
        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(MockSpeechServer::url()),
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
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
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
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            "voicevox",
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unknown kind rejected");
    assert!(matches!(err, PluginError::NotSupported(_)));
}

#[tokio::test]
async fn out_of_range_speed_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockSpeechServer::url(),
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

#[tokio::test]
async fn rate_limit_retries_then_succeeds() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(
        MockResponse::with_status(429, br#"{"error":"slow down"}"#.to_vec())
            .with_header("Retry-After", "0"),
    );
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("retry succeeds");
    assert_wav_payload(&audio, &pcm_fixture());
    assert_eq!(mock.requests.lock().expect("request log").len(), 2);
}

#[tokio::test]
async fn empty_audio_response_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(Vec::new()));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("empty audio rejected");
    assert!(err.to_string().contains("empty"));
}

#[tokio::test]
async fn input_longer_than_the_api_limit_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "a".repeat(4097),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("overlong input rejected");
    assert!(err.to_string().contains("4096"));
    assert!(mock.requests.lock().expect("request log").is_empty());

    mock.push(MockResponse::ok(pcm_fixture()));
    test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "a".repeat(4096),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("4096-character input is accepted");
}

#[tokio::test]
async fn unknown_voice_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(MockSpeechServer::url()),
            "hello".to_string(),
            "clippy".to_string(),
            "wav".to_string(),
        )
        .await
        .expect_err("unknown voice rejected");
    assert!(err.to_string().contains("unknown voice"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn unknown_model_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockSpeechServer::url(),
                "model": "tts-2"
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unknown model rejected");
    assert!(err.to_string().contains("model"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn configured_sample_rate_is_written_into_the_wav_header() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockSpeechServer::url(),
                "sample_rate": 48_000
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_eq!(
        u32::from_le_bytes([audio[24], audio[25], audio[26], audio[27]]),
        48_000
    );
    assert_eq!(
        u32::from_le_bytes([audio[28], audio[29], audio[30], audio[31]]),
        96_000
    );
}

#[tokio::test]
async fn base_url_trailing_slash_is_normalized() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": "https://api.openai.com/v1/"
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("trailing slash tolerated");

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests[0].url, "https://api.openai.com/v1/audio/speech");
}

#[tokio::test]
async fn set_config_base_url_is_used_by_synthesis() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    test_plugin().set_config(&json!({"base_url": "https://host.example/v1"}));
    let mock = MockSpeechServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({"base_url": "https://request.example/v1"}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests[0].url, "https://host.example/v1/audio/speech");
}

#[test]
fn base_url_env_falls_back_after_config() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("OPENAI_BASE_URL").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
    }
    assert_eq!(resolve_base_url(None, &json!({})), "https://env.example/v1");
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("OPENAI_BASE_URL", value);
        },
        None => unsafe {
            std::env::remove_var("OPENAI_BASE_URL");
        },
    }
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
    assert_eq!(schema["properties"]["sample_rate"]["default"], 24_000);
    assert_eq!(schema["properties"]["sample_rate"]["minimum"], 1);
}

#[test]
fn base_url_precedence_and_default() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("OPENAI_BASE_URL").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("OPENAI_BASE_URL");
    }
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
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("OPENAI_BASE_URL", value);
        },
        None => unsafe {
            std::env::remove_var("OPENAI_BASE_URL");
        },
    }
}
