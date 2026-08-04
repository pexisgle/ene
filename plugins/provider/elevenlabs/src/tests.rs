//! Plugin tests: synthesis against in-process fake `ElevenLabs` REST and
//! WebSocket servers, error paths, and capability/schema coverage.

#![expect(
    clippy::await_holding_lock,
    clippy::expect_used,
    reason = "tests serialize the process-wide PLUGIN_CONFIG across synthesis calls \
              and use expect for concise assertions"
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, PoisonError};
use std::time::Duration;

use base64::Engine;
use ene_plugin::{ConfigurablePlugin as _, PluginError, ProviderErrorKind, TtsPlugin as _};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use http::Request as HttpRequest;
use serde_json::{Value, json};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, accept_hdr_async};

use crate::client::{MAX_ERROR_BODY_BYTES, MAX_PCM_BYTES};
use crate::config::{DEFAULT_BASE_URL, DEFAULT_MODEL, resolve_api_key, resolve_base_url};
use crate::mock_server::{MockElevenLabsServer, MockResponse};
use crate::plugin::{ElevenLabsPlugin, PLUGIN_CONFIG};

const KIND: &str = "elevenlabs";
const TEST_KEY: &str = "xi-test-123";
const TEST_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";

/// Serializes tests that read or write the process-wide `PLUGIN_CONFIG`
/// static (every synthesize call reads it).
static TEST_SERIAL: StdMutex<()> = StdMutex::new(());

/// Serializes tests that mutate the process environment.
static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

fn test_plugin() -> ElevenLabsPlugin {
    ElevenLabsPlugin
}

fn config_json(base_url: &str) -> Value {
    json!({
        "api_key": TEST_KEY,
        "base_url": base_url,
        "mode": "rest",
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

fn mock_url(mock: &MockElevenLabsServer) -> String {
    format!("{}/v1", mock.url)
}

#[tokio::test]
async fn synthesis_returns_wav_and_sends_expected_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    assert_wav_payload(&audio, &pcm, 24_000);

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path,
        format!("/v1/text-to-speech/{TEST_VOICE}/stream?output_format=pcm_24000")
    );
    assert_eq!(request_header(request, "xi-api-key"), Some(TEST_KEY));
    assert!(
        request_header(request, "content-type")
            .is_some_and(|value| value.starts_with("application/json"))
    );
    let body: Value = serde_json::from_str(&request.body).expect("request body is JSON");
    assert_eq!(body["text"], "こんにちは");
    assert_eq!(body["model_id"], "eleven_turbo_v2_5");
    assert!(
        (body["voice_settings"]["stability"]
            .as_f64()
            .expect("number")
            - 0.5)
            .abs()
            < 1e-6
    );
    assert!(
        (body["voice_settings"]["similarity_boost"]
            .as_f64()
            .expect("number")
            - 0.75)
            .abs()
            < 1e-6
    );
    assert_eq!(body["voice_settings"]["use_speaker_boost"], true);
}

#[tokio::test]
async fn per_request_voice_overrides_configured_voice() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            "EXAVITQu4vr4xnSDxMaL".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0]
            .path
            .contains("/text-to-speech/EXAVITQu4vr4xnSDxMaL/stream")
    );
}

#[tokio::test]
async fn omitted_settings_fall_back_to_defaults() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "voice_id": TEST_VOICE
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    assert_eq!(
        requests[0].path,
        format!("/v1/text-to-speech/{TEST_VOICE}/stream?output_format=pcm_24000")
    );
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    assert_eq!(body["model_id"], DEFAULT_MODEL);
    assert!(
        (body["voice_settings"]["stability"]
            .as_f64()
            .expect("number")
            - 0.5)
            .abs()
            < 1e-6
    );
    assert!(
        (body["voice_settings"]["similarity_boost"]
            .as_f64()
            .expect("number")
            - 0.75)
            .abs()
            < 1e-6
    );
    assert!(
        body["voice_settings"]["style"]
            .as_f64()
            .expect("number")
            .abs()
            < 1e-6
    );
    assert_eq!(body["voice_settings"]["use_speaker_boost"], true);
}

#[tokio::test]
async fn streamed_chunked_response_is_accumulated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    assert_wav_payload(&audio, &pcm, 24_000);
}

#[tokio::test]
async fn auth_errors_map_to_typed_auth_without_echoing_the_key() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    for status in [401, 403] {
        let mock = MockElevenLabsServer::spawn().expect("mock server");
        mock.push(MockResponse::with_status(
            status,
            b"{\"detail\":{\"status\":401,\"message\":\"bad key\"}}".to_vec(),
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    for _ in 0..3 {
        mock.push(
            MockResponse::with_status(429, b"{\"detail\":\"slow down\"}".to_vec())
                .with_header("Retry-After", "0"),
        );
    }

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
    assert!(!err.to_string().contains(TEST_KEY));
}

#[tokio::test]
async fn server_error_includes_status_and_detail_message() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::with_status(
        500,
        b"{\"detail\":{\"status\":500,\"message\":\"upstream exploded\"}}".to_vec(),
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
async fn string_detail_is_surfaced() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::with_status(
        400,
        b"{\"detail\":\"voice not found: 21m00Tcm4TlvDq8ikWAM\"}".to_vec(),
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
        .expect_err("bad request surfaces");
    assert!(err.to_string().contains("voice not found"));
}

#[tokio::test]
async fn truncated_pcm_maps_to_typed_truncated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
    let mock = MockElevenLabsServer::spawn().expect("mock server");
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
async fn missing_voice_id_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "voice_id": ""
            }),
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
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            "../admin".to_string(),
            "wav".to_string(),
        )
        .await
        .expect_err("unsafe voice rejected");
    assert!(err.to_string().contains("voice_id"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn missing_api_key_fails_without_hitting_the_network() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("ELEVENLABS_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("ELEVENLABS_API_KEY");
    }
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "base_url": mock_url(&mock),
                "voice_id": TEST_VOICE
            }),
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
            std::env::set_var("ELEVENLABS_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("ELEVENLABS_API_KEY");
        },
    }
}

#[tokio::test]
async fn out_of_range_sample_rate_is_rejected_before_the_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    let err = test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "voice_id": TEST_VOICE,
                "sample_rate": 48_000
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("unsupported sample rate rejected");
    assert!(err.to_string().contains("sample_rate"));
    assert!(mock.requests.lock().expect("request log").is_empty());
}

#[tokio::test]
async fn rate_limit_retries_then_succeeds() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(
        MockResponse::with_status(429, b"{\"detail\":\"slow down\"}".to_vec())
            .with_header("Retry-After", "0"),
    );
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
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
async fn empty_audio_response_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(Vec::new()));

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
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
    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "a".repeat(5001),
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
            config_json(&mock_url(&mock)),
            "a".repeat(5000),
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
    mock.push(MockResponse::ok(pcm_fixture()));

    let audio = test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "voice_id": TEST_VOICE,
                "sample_rate": 44_100
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_eq!(
        u32::from_le_bytes([audio[24], audio[25], audio[26], audio[27]]),
        44_100
    );
    assert_eq!(
        u32::from_le_bytes([audio[28], audio[29], audio[30], audio[31]]),
        88_200
    );
    let requests = mock.requests.lock().expect("request log");
    assert!(
        requests[0]
            .path
            .ends_with("/stream?output_format=pcm_44100"),
        "path: {}",
        requests[0].path
    );
}

#[tokio::test]
async fn base_url_trailing_slash_is_normalized() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": format!("{}/v1/", mock.url),
                "voice_id": TEST_VOICE
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("trailing slash tolerated");

    let requests = mock.requests.lock().expect("request log");
    assert!(requests[0].path.starts_with("/v1/text-to-speech/"));
}

#[tokio::test]
async fn oversized_error_body_is_bounded() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(
        MockResponse::with_status(500, b"boom".to_vec())
            .with_declared_length(MAX_ERROR_BODY_BYTES + 1),
    );

    let err = test_plugin()
        .synthesize(
            KIND,
            config_json(&mock_url(&mock)),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized error body bounded");
    assert!(err.to_string().contains("too large"));
}

#[tokio::test]
async fn voice_settings_are_clamped_before_sending() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let mock = MockElevenLabsServer::spawn().expect("mock server");
    mock.push(MockResponse::ok(pcm_fixture()));

    test_plugin()
        .synthesize(
            KIND,
            json!({
                "api_key": TEST_KEY,
                "base_url": mock_url(&mock),
                "voice_id": TEST_VOICE,
                "voice_settings": {
                    "stability": 1.5,
                    "similarity_boost": -0.5,
                    "style": 2.0,
                    "use_speaker_boost": false
                }
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("clamped synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    let body: Value = serde_json::from_str(&requests[0].body).expect("request body is JSON");
    assert!(
        (body["voice_settings"]["stability"]
            .as_f64()
            .expect("number")
            - 1.0)
            .abs()
            < 1e-6
    );
    assert!(
        body["voice_settings"]["similarity_boost"]
            .as_f64()
            .expect("number")
            .abs()
            < 1e-6
    );
    assert!((body["voice_settings"]["style"].as_f64().expect("number") - 1.0).abs() < 1e-6);
    assert_eq!(body["voice_settings"]["use_speaker_boost"], false);
}

fn ws_config(addr: SocketAddr) -> Value {
    json!({
        "api_key": TEST_KEY,
        "base_url": format!("http://{addr}/v1"),
        "mode": "ws",
        "model_id": "eleven_turbo_v2_5",
        "voice_id": TEST_VOICE,
        "sample_rate": 24_000
    })
}

/// Spawns a mock WebSocket server; `serve` runs once per accepted
/// connection with the 1-based connection index.
async fn spawn_ws_server<F, Fut>(serve: F) -> (SocketAddr, Arc<StdMutex<Vec<String>>>)
where
    F: Fn(usize, WebSocketStream<TcpStream>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let task_requests = Arc::clone(&requests);
    tokio::spawn(async move {
        let mut index = 0usize;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let requests = Arc::clone(&task_requests);
            // tungstenite's handshake callback returns `Response` in both
            // variants, which is large; boxing is not possible without
            // changing the trait.
            #[expect(
                clippy::result_large_err,
                reason = "tungstenite's handshake callback returns Response in both variants"
            )]
            let ws = accept_hdr_async(stream, move |request: &HttpRequest<()>, response| {
                if let Ok(mut guard) = requests.lock() {
                    guard.push(request.uri().to_string());
                }
                Ok(response)
            })
            .await
            .expect("handshake");
            index += 1;
            tokio::spawn(serve(index, ws));
        }
    });
    (addr, requests)
}

/// Reads one text frame.
async fn next_text(stream: &mut SplitStream<WebSocketStream<TcpStream>>) -> Result<String, String> {
    match stream.next().await {
        Some(Ok(Message::Text(text))) => Ok(text.to_string()),
        Some(Ok(_)) => Err("expected a text frame".to_string()),
        Some(Err(e)) => Err(e.to_string()),
        None => Err("stream closed".to_string()),
    }
}

/// Collects `{"text": …}` frames until the terminal empty-text frame,
/// returning the payloads in order.
async fn collect_text_frames(stream: &mut SplitStream<WebSocketStream<TcpStream>>) -> Vec<String> {
    let mut frames = Vec::new();
    loop {
        let frame = next_text(stream).await.expect("text frame");
        let value: Value = serde_json::from_str(&frame).expect("frame is JSON");
        if value["text"] == "" {
            return frames;
        }
        frames.push(value["text"].as_str().expect("text field").to_string());
    }
}

/// Sends `pcm` as two base64 audio frames followed by `isFinal: true`.
async fn send_audio_and_final(
    sink: &mut SplitSink<WebSocketStream<TcpStream>, Message>,
    pcm: &[u8],
    final_key: &str,
) {
    let (first, second) = pcm.split_at(4);
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    sink.send(Message::text(
        json!({ "audio": encode(first), "isFinal": false }).to_string(),
    ))
    .await
    .expect("send audio 1");
    sink.send(Message::text(
        json!({ "audio": encode(second), "isFinal": false }).to_string(),
    ))
    .await
    .expect("send audio 2");
    sink.send(Message::text(json!({ final_key: true }).to_string()))
        .await
        .expect("send final");
}

async fn serve_success(ws: WebSocketStream<TcpStream>, pcm: &[u8], final_key: &str) {
    let (mut sink, mut stream) = ws.split();
    let init = next_text(&mut stream).await.expect("init frame");
    let init_value: Value = serde_json::from_str(&init).expect("init is JSON");
    assert_eq!(init_value["text"], " ");
    assert!(init_value["voice_settings"].is_object());
    assert!(init_value["generation_config"].is_object());
    let frames = collect_text_frames(&mut stream).await;
    assert!(!frames.is_empty());
    send_audio_and_final(&mut sink, pcm, final_key).await;
}

#[tokio::test]
async fn ws_mode_synthesizes_over_websocket() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let expected_pcm = pcm.clone();
    let (addr, requests) = spawn_ws_server(move |index, ws| {
        let pcm = pcm.clone();
        async move {
            assert_eq!(index, 1);
            serve_success(ws, &pcm, "isFinal").await;
        }
    })
    .await;

    let audio = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello. World!".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("ws synthesis succeeds");
    assert_wav_payload(&audio, &expected_pcm, 24_000);

    let requests = requests.lock().expect("request log");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].contains(&format!("/v1/text-to-speech/{TEST_VOICE}/stream-input")),
        "request: {}",
        requests[0]
    );
    assert!(requests[0].contains("model_id=eleven_turbo_v2_5"));
    assert!(requests[0].contains("output_format=pcm_24000"));
}

#[tokio::test]
async fn ws_mode_accepts_snake_case_final_marker() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let expected_pcm = pcm.clone();
    let (addr, _) = spawn_ws_server(move |_, ws| {
        let pcm = pcm.clone();
        async move {
            serve_success(ws, &pcm, "is_final").await;
        }
    })
    .await;

    let audio = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("snake-case final accepted");
    assert_wav_payload(&audio, &expected_pcm, 24_000);
}

#[tokio::test]
async fn ws_server_error_is_terminal_and_not_retried() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let connections = Arc::new(AtomicUsize::new(0));
    let task_connections = Arc::clone(&connections);
    let (addr, _) = spawn_ws_server(move |_, ws| {
        let connections = Arc::clone(&task_connections);
        async move {
            connections.fetch_add(1, Ordering::SeqCst);
            let (mut sink, mut stream) = ws.split();
            let _init = next_text(&mut stream).await.expect("init frame");
            let _frames = collect_text_frames(&mut stream).await;
            sink.send(Message::text(
                json!({ "error": { "status": 429, "message": "quota exceeded" } }).to_string(),
            ))
            .await
            .expect("send error");
        }
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("server error surfaces");
    assert_eq!(
        err.provider_error_kind(),
        Some(ProviderErrorKind::RateLimit)
    );
    assert!(err.to_string().contains("quota exceeded"));
    assert!(!err.to_string().contains(TEST_KEY));
    // A terminal error must not spend the retry budget: allow a full backoff
    // window to pass and assert no second connection was made.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ws_transport_failure_retries_the_whole_request() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let connections = Arc::new(AtomicUsize::new(0));
    let task_connections = Arc::clone(&connections);
    let pcm = pcm_fixture();
    let expected_pcm = pcm.clone();
    let (addr, _) = spawn_ws_server(move |index, ws| {
        let connections = Arc::clone(&task_connections);
        let pcm = pcm.clone();
        async move {
            connections.fetch_add(1, Ordering::SeqCst);
            let (mut sink, mut stream) = ws.split();
            let _init = next_text(&mut stream).await.expect("init frame");
            let _frames = collect_text_frames(&mut stream).await;
            if index == 1 {
                // Abrupt close before any audio or final frame.
                drop(sink);
            } else {
                send_audio_and_final(&mut sink, &pcm, "isFinal").await;
            }
        }
    })
    .await;

    let audio = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello. World!".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("retry succeeds");
    assert_wav_payload(&audio, &expected_pcm, 24_000);
    assert_eq!(connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn ws_handshake_401_maps_to_auth() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = Vec::new();
        let mut temp = [0u8; 4096];
        loop {
            let read = stream.read(&mut temp).await.expect("read");
            if read == 0 {
                break;
            }
            buf.extend_from_slice(&temp[..read]);
            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let head = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        stream.write_all(head.as_bytes()).await.expect("write");
        stream.flush().await.expect("flush");
    });

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("handshake rejection surfaces");
    assert_eq!(err.provider_error_kind(), Some(ProviderErrorKind::Auth));
}

#[tokio::test]
async fn ws_non_json_message_is_an_error() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (addr, _) = spawn_ws_server(|_, ws| async move {
        let (mut sink, mut stream) = ws.split();
        let _init = next_text(&mut stream).await.expect("init frame");
        let _frames = collect_text_frames(&mut stream).await;
        sink.send(Message::text("not json".to_string()))
            .await
            .expect("send garbage");
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("malformed frame surfaces");
    assert!(err.to_string().contains("invalid message"));
}

#[tokio::test]
async fn ws_invalid_base64_audio_is_an_error() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (addr, _) = spawn_ws_server(|_, ws| async move {
        let (mut sink, mut stream) = ws.split();
        let _init = next_text(&mut stream).await.expect("init frame");
        let _frames = collect_text_frames(&mut stream).await;
        sink.send(Message::text(
            json!({ "audio": "!!!not-base64!!!", "isFinal": false }).to_string(),
        ))
        .await
        .expect("send bad audio");
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("bad base64 surfaces");
    assert!(err.to_string().contains("invalid base64"));
}

#[tokio::test]
async fn ws_empty_audio_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (addr, _) = spawn_ws_server(|_, ws| async move {
        let (mut sink, mut stream) = ws.split();
        let _init = next_text(&mut stream).await.expect("init frame");
        let _frames = collect_text_frames(&mut stream).await;
        sink.send(Message::text(json!({ "isFinal": true }).to_string()))
            .await
            .expect("send final");
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("empty audio surfaces");
    assert!(err.to_string().contains("empty"));
}

#[tokio::test]
async fn ws_odd_pcm_maps_to_typed_truncated() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (addr, _) = spawn_ws_server(|_, ws| async move {
        let (mut sink, mut stream) = ws.split();
        let _init = next_text(&mut stream).await.expect("init frame");
        let _frames = collect_text_frames(&mut stream).await;
        let audio = base64::engine::general_purpose::STANDARD.encode([0u8, 0, 1]);
        sink.send(Message::text(
            json!({ "audio": audio, "isFinal": true }).to_string(),
        ))
        .await
        .expect("send odd audio");
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("odd PCM surfaces");
    assert_eq!(
        err.provider_error_kind(),
        Some(ProviderErrorKind::Truncated)
    );
}

#[tokio::test]
async fn ws_audio_over_the_cap_is_rejected() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (addr, _) = spawn_ws_server(|_, ws| async move {
        let (mut sink, mut stream) = ws.split();
        let _init = next_text(&mut stream).await.expect("init frame");
        let _frames = collect_text_frames(&mut stream).await;
        let chunk = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1024 * 1024]);
        for _ in 0..33 {
            if sink
                .send(Message::text(
                    json!({ "audio": &chunk, "isFinal": false }).to_string(),
                ))
                .await
                .is_err()
            {
                // The client dropped the connection after rejecting the
                // oversized stream; nothing more to send.
                return;
            }
        }
    })
    .await;

    let err = test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("oversized ws audio rejected");
    assert!(err.to_string().contains("exceeds"));
}

#[tokio::test]
async fn ws_request_voice_is_used_in_the_websocket_url() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let pcm = pcm_fixture();
    let (addr, requests) = spawn_ws_server(move |_, ws| {
        let pcm = pcm.clone();
        async move {
            serve_success(ws, &pcm, "isFinal").await;
        }
    })
    .await;

    test_plugin()
        .synthesize(
            KIND,
            ws_config(addr),
            "Hello".to_string(),
            "EXAVITQu4vr4xnSDxMaL".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("ws synthesis succeeds");

    let requests = requests.lock().expect("request log");
    assert!(
        requests[0].contains("/text-to-speech/EXAVITQu4vr4xnSDxMaL/stream-input"),
        "request: {}",
        requests[0]
    );
}

#[test]
fn capabilities_declare_kind_formats_and_concurrency() {
    let capabilities = test_plugin().tts_capabilities();
    assert_eq!(capabilities.len(), 1);
    let spec = &capabilities[0];
    assert_eq!(spec.kind, KIND);
    // ElevenLabs voices are user-specific, so there is no closed list.
    assert!(spec.voices.is_empty());
    assert_eq!(spec.formats, vec!["wav"]);
    assert_eq!(spec.concurrency.max_in_flight, 8);
    assert_eq!(spec.concurrency.queue_depth, 16);
}

#[test]
fn config_schema_marks_api_key_secret_and_constrains_settings() {
    let schema = test_plugin().config_schema().expect("schema present");
    assert_eq!(schema["properties"]["api_key"]["x-ene-secret"], true);
    assert_eq!(schema["properties"]["mode"]["enum"], json!(["rest", "ws"]));
    assert_eq!(schema["properties"]["mode"]["default"], "rest");
    assert_eq!(
        schema["properties"]["sample_rate"]["enum"],
        json!([16_000, 24_000, 44_100])
    );
    assert_eq!(schema["properties"]["sample_rate"]["default"], 24_000);
    assert_eq!(schema["properties"]["model_id"]["default"], DEFAULT_MODEL);
    assert_eq!(
        schema["properties"]["voice_settings"]["properties"]["stability"]["maximum"],
        1.0
    );
    assert_eq!(
        schema["properties"]["voice_settings"]["properties"]["stability"]["minimum"],
        0.0
    );
}

#[test]
fn set_config_key_is_used_by_synthesis_resolution() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    test_plugin().set_config(&json!({"api_key": "xi-via-set-config"}));

    let host = PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    let resolved = resolve_api_key(host.as_ref(), &json!({})).expect("key resolves");
    assert_eq!(resolved, "xi-via-set-config");
    drop(host);

    *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

#[test]
fn resolves_plain_string_keys() {
    let config = json!({"api_key": "xi-plain-789"});
    assert_eq!(
        resolve_api_key(None, &config).expect("plain key resolves"),
        "xi-plain-789"
    );
}

#[test]
fn resolves_inline_descriptor() {
    let config = json!({"api_key": {"source": "inline", "inline": "xi-inline-123"}});
    assert_eq!(
        resolve_api_key(None, &config).expect("inline key resolves"),
        "xi-inline-123"
    );
}

#[test]
fn resolves_env_descriptor() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::set_var("ENE_TEST_ELEVENLABS_KEY", "xi-env-test-456");
    }
    let config = json!({"api_key": {"source": "env", "env": "ENE_TEST_ELEVENLABS_KEY"}});
    assert_eq!(
        resolve_api_key(None, &config).expect("env key resolves"),
        "xi-env-test-456"
    );
    // SAFETY: test-only cleanup, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("ENE_TEST_ELEVENLABS_KEY");
    }
}

#[test]
fn host_config_key_wins_over_request_config() {
    let host = json!({"api_key": "xi-host-wins"});
    let request = json!({"api_key": "xi-request-loses"});
    assert_eq!(
        resolve_api_key(Some(&host), &request).expect("host key wins"),
        "xi-host-wins"
    );
}

#[test]
fn process_env_is_the_last_fallback() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("ELEVENLABS_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::set_var("ELEVENLABS_API_KEY", "xi-auto-000");
    }
    assert_eq!(
        resolve_api_key(None, &json!({})).expect("env fallback resolves"),
        "xi-auto-000"
    );
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("ELEVENLABS_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("ELEVENLABS_API_KEY");
        },
    }
}

#[test]
fn missing_key_everywhere_is_an_error() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
    let original = std::env::var("ELEVENLABS_API_KEY").ok();
    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var("ELEVENLABS_API_KEY");
    }
    let err = resolve_api_key(None, &json!({})).expect_err("no key anywhere");
    assert!(err.to_string().contains("no API key"));
    // SAFETY: test-only env restore, serialized by `ENV_MUTEX`.
    match original {
        Some(value) => unsafe {
            std::env::set_var("ELEVENLABS_API_KEY", value);
        },
        None => unsafe {
            std::env::remove_var("ELEVENLABS_API_KEY");
        },
    }
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
