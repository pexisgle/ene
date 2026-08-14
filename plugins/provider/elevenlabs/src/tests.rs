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
use crate::config::{DEFAULT_BASE_URL, DEFAULT_MODEL, resolve_base_url};
use crate::mock_server::{MockElevenLabsServer, MockResponse, RecordedRequest};
use crate::plugin::{ElevenLabsPlugin, PLUGIN_CONFIG};

const KIND: &str = "elevenlabs";
const TEST_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";

/// Serializes tests that read or write the process-wide `PLUGIN_CONFIG`
/// static or the shared broker (every synthesize call touches both).
static TEST_SERIAL: StdMutex<()> = StdMutex::new(());

/// Serializes tests that mutate the process environment.
static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

fn test_plugin() -> ElevenLabsPlugin {
    ElevenLabsPlugin
}

/// Points the shared broker at `mock`'s socket. The static broker caches
/// its session, so each test resets it first; `TEST_SERIAL` keeps the
/// shared static from racing across tests.
async fn configure_broker(mock: &MockElevenLabsServer) {
    broker().reset().await;
    broker().configure(&SandboxConfigData {
        broker_socket: Some(mock.socket_path().to_string_lossy().into_owned()),
        db_auth_token: Some("tok".to_string()),
        ..SandboxConfigData::default()
    });
}

fn config_json() -> Value {
    json!({
        "base_url": MockElevenLabsServer::url(),
        "model_id": "eleven_turbo_v2_5",
        "voice_id": TEST_VOICE,
        "sample_rate": 24_000
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
/// `sample_rate`.
fn assert_wav_payload(wav: &[u8], expected_pcm: &[u8], sample_rate: u32) {
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(
        u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
        sample_rate
    );
    assert_eq!(&wav[44..], expected_pcm);
}

#[tokio::test]
async fn synthesis_returns_wav_and_sends_expected_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm.clone()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_wav_payload(&audio, &pcm, 24_000);

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.url,
        format!(
            "{}/text-to-speech/{TEST_VOICE}/stream?output_format=pcm_24000",
            MockElevenLabsServer::url()
        )
    );
    // The plugin names the host-owned credential instead of sending a key.
    assert_eq!(request.credential.as_deref(), Some("api_key"));
    assert_eq!(request.credential_header.as_deref(), Some("xi-api-key"));
    assert!(
        request_header(request, "content-type")
            .is_some_and(|value| value.starts_with("application/json"))
    );
    assert!(
        request_header(request, "xi-api-key").is_none(),
        "the plugin must not send the credential itself"
    );
    let body: Value = serde_json::from_str(&request.body).expect("request body is JSON");
    assert_eq!(body["model_id"], "eleven_turbo_v2_5");
    assert_eq!(body["text"], "hello");
}

#[tokio::test]
async fn per_request_voice_overrides_configured_voice() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            "custom_voice".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0]
            .url
            .contains("/text-to-speech/custom_voice/stream")
    );
}

#[tokio::test]
async fn omitted_settings_fall_back_to_defaults() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockElevenLabsServer::url(),
                "voice_id": TEST_VOICE
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    assert_eq!(body["model_id"], DEFAULT_MODEL);
    assert_eq!(
        requests[0].url,
        format!(
            "{}/text-to-speech/{TEST_VOICE}/stream?output_format=pcm_24000",
            MockElevenLabsServer::url()
        )
    );
}

#[tokio::test]
async fn auth_errors_map_to_typed_auth() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    for status in [401, 403] {
        let mock = MockElevenLabsServer::spawn().expect("mock server");
        configure_broker(&mock).await;
        mock.push(MockResponse::with_status(
            status,
            br#"{"detail":"bad key"}"#.to_vec(),
        ));

        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(),
                "hello".to_string(),
                String::new(),
                "wav".to_string(),
            )
            .await
            .expect_err("auth failure surfaces");
        assert_eq!(err.provider_error_kind(), Some(ProviderErrorKind::Auth));
        assert!(err.to_string().contains("bad key"));
    }
}

#[tokio::test]
async fn rate_limit_maps_to_typed_rate_limit() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for _ in 0..3 {
        mock.push(
            MockResponse::with_status(429, br#"{"detail":"slow down"}"#.to_vec())
                .with_header("Retry-After", "0"),
        );
    }

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
async fn server_error_includes_status_and_detail_message() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for _ in 0..3 {
        mock.push(MockResponse::with_status(
            500,
            br#"{"detail":{"status":500,"message":"upstream exploded"}}"#.to_vec(),
        ));
    }

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
async fn string_detail_is_surfaced() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::with_status(
        422,
        br#"{"detail":"string detail"}"#.to_vec(),
    ));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("string detail surfaces");
    assert!(err.to_string().contains("string detail"));
}

#[tokio::test]
async fn truncated_pcm_maps_to_typed_truncated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(vec![0, 0, 1]));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(vec![0u8; MAX_PCM_BYTES + 2]));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized payload rejected");
    assert!(err.to_string().contains("exceeds"));
}

#[tokio::test]
async fn rejects_unsupported_formats() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for format in ["mp3", "pcm"] {
        let err = test_plugin()
            .synthesize(
                KIND,
                config_json(),
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            "voicevox",
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unknown kind rejected");
    assert!(matches!(err, PluginError::NotSupported(_)));
}

#[tokio::test]
async fn missing_voice_id_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({ "base_url": MockElevenLabsServer::url() }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("missing voice rejected");
    assert!(err.to_string().contains("voice"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn unsafe_request_voice_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            "../escape".to_string(),
            "wav".to_string(),
        )
        .await
        .expect_err("unsafe voice rejected");
    assert!(err.to_string().contains("voice"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn out_of_range_sample_rate_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockElevenLabsServer::url(),
                "voice_id": TEST_VOICE,
                "sample_rate": 48_000
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unsupported rate rejected");
    assert!(err.to_string().contains("sample_rate"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn rate_limit_retries_then_succeeds() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(
        MockResponse::with_status(429, br#"{"detail":"slow down"}"#.to_vec())
            .with_header("Retry-After", "0"),
    );
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("retry succeeds");
    assert_wav_payload(&audio, &pcm_fixture(), 24_000);
    assert_eq!(mock.requests.lock().expect("request log").len(), 2);
}

#[tokio::test]
async fn rate_limit_http_date_retry_after_is_honored() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    // An HTTP-date in the past resolves to a zero delay, keeping the test
    // fast while exercising the RFC 9110 date parsing path.
    let past = chrono::Utc::now() - chrono::Duration::seconds(5);
    let date = past.to_rfc2822();
    mock.push(
        MockResponse::with_status(429, br#"{"detail":"slow down"}"#.to_vec())
            .with_header("Retry-After", &date),
    );
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("date retry-after succeeds");
    assert_eq!(mock.requests.lock().expect("request log").len(), 2);
}

#[tokio::test]
async fn oversized_rate_limit_error_body_still_retries() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::with_status(429, vec![b'x'; 200_000]).with_header("Retry-After", "0"));
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("retry succeeds despite the oversized error body");
    assert_eq!(mock.requests.lock().expect("request log").len(), 2);
}

#[tokio::test]
async fn server_error_retries_then_succeeds() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::with_status(
        503,
        br#"{"detail":"temporary"}"#.to_vec(),
    ));
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("retry succeeds");
    assert_eq!(mock.requests.lock().expect("request log").len(), 2);
}

#[tokio::test]
async fn empty_audio_response_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(Vec::new()));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "a".repeat(5_001),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("overlong input rejected");
    assert!(err.to_string().contains("5000"));
    assert!(mock.requests.lock().expect("request log").is_empty());

    mock.push(MockResponse::ok(pcm_fixture()));
    test_plugin()
        .synthesize(
            KIND,
            config_json(),
            "a".repeat(5_000),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("5000-character input is accepted");
}

#[tokio::test]
async fn configured_sample_rate_selects_format_and_wav_header() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockElevenLabsServer::url(),
                "voice_id": TEST_VOICE,
                "sample_rate": 44_100
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_wav_payload(&audio, &pcm_fixture(), 44_100);
    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0].url.contains("output_format=pcm_44100"),
        "url must select the configured rate: {}",
        requests[0].url
    );
}

#[tokio::test]
async fn base_url_trailing_slash_is_normalized() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": format!("{}/", MockElevenLabsServer::url()),
                "voice_id": TEST_VOICE
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("trailing slash tolerated");

    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0]
            .url
            .starts_with(&format!("{}/text-to-speech/", MockElevenLabsServer::url())),
        "no double slash: {}",
        requests[0].url
    );
}

#[tokio::test]
async fn oversized_error_body_is_bounded() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    for _ in 0..3 {
        mock.push(MockResponse::with_status(500, b"boom-".repeat(20_000)));
    }

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(),
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
async fn voice_settings_are_clamped_before_sending() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockElevenLabsServer::url(),
                "voice_id": TEST_VOICE,
                "voice_settings": {
                    "stability": 1.5,
                    "similarity_boost": -0.5,
                    "style": 2.0
                }
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("clamped settings accepted");

    let requests = mock.requests.lock().expect("request log");
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    let settings = &body["voice_settings"];
    assert_eq!(settings["stability"], 1.0);
    assert_eq!(settings["similarity_boost"], 0.0);
    assert_eq!(settings["style"], 1.0);
}

#[tokio::test]
async fn set_config_base_url_is_used_by_synthesis() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    test_plugin().set_config(&json!({"base_url": "https://host.example/v1"}));
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    configure_broker(&mock).await;
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": MockElevenLabsServer::url(),
                "voice_id": TEST_VOICE
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;

    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0]
            .url
            .starts_with("https://host.example/v1/text-to-speech/"),
        "host blob base_url must win: {}",
        requests[0].url
    );
}

#[test]
fn capabilities_declare_kind_formats_and_concurrency() {
    let capabilities = test_plugin().tts_capabilities();
    assert_eq!(capabilities.len(), 1);
    let spec = &capabilities[0];
    assert_eq!(spec.kind, KIND);
    assert_eq!(spec.formats, vec!["wav"]);
    assert_eq!(spec.concurrency.max_in_flight, 8);
    assert_eq!(spec.concurrency.queue_depth, 16);
}

#[test]
fn config_schema_marks_api_key_secret_and_constrains_settings() {
    let schema = test_plugin().config_schema().expect("schema present");
    assert_eq!(schema["properties"]["api_key"]["x-ene-secret"], true);
    assert_eq!(schema["properties"]["model_id"]["default"], DEFAULT_MODEL);
    assert_eq!(schema["properties"]["sample_rate"]["enum"][0], 16_000);
}

#[test]
fn base_url_precedence_and_default() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("ELEVENLABS_BASE_URL").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("ELEVENLABS_BASE_URL");
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
            std::env::set_var("ELEVENLABS_BASE_URL", value);
        },
        None => unsafe {
            std::env::remove_var("ELEVENLABS_BASE_URL");
        },
    }
}
