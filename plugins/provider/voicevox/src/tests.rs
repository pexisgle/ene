//! Plugin tests: external-mode 2-step flow against an in-process fake
//! engine, error paths, and managed-mode lifecycle against a real
//! `voicevox-fake-engine` child process.

#![expect(
    clippy::expect_used,
    reason = "unit tests use expect/expect_err for concise assertions"
)]

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ene_plugin::{ConfigurablePlugin as _, TtsPlugin as _};
use serde_json::{Value, json};

use crate::config::VoicevoxConfig;
use crate::mock_engine::spawn_mock_engine;
use crate::plugin::VoicevoxPlugin;

/// Serializes tests that mutate the process environment: `set_var` /
/// `remove_var` are process-global and would otherwise race with concurrent
/// env reads in other tests.
static ENV_MUTEX: StdMutex<()> = StdMutex::new(());

const KIND: &str = "voicevox";

fn test_plugin() -> VoicevoxPlugin {
    VoicevoxPlugin::default()
}

fn config_json(server_url: &str) -> Value {
    json!({
        "server_url": server_url,
        "speaker_id": 3,
        "speed_scale": 1.3
    })
}

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe listener")
        .local_addr()
        .expect("probe address")
        .port()
}

#[tokio::test]
async fn external_mode_two_step_synthesis_returns_wav() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();

    let wav = plugin
        .synthesize(
            KIND,
            config_json(&mock.url),
            "こんにちは".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    assert!(wav.starts_with(b"RIFF"));
    assert!(wav.len() > 44);
    let requests = mock.requests.lock().expect("request log");
    assert_eq!(requests.len(), 2);
    let query = &requests[0];
    assert_eq!(query.method, "POST");
    assert!(query.path.starts_with("/audio_query?"));
    assert!(query.path.contains("text="));
    assert!(query.path.contains("speaker=3"));
    let synthesis = &requests[1];
    assert_eq!(synthesis.method, "POST");
    assert!(synthesis.path.starts_with("/synthesis?"));
    assert!(synthesis.path.contains("speaker=3"));
    let body: Value = serde_json::from_str(&synthesis.body).expect("query body is JSON");
    assert!((body["speedScale"].as_f64().expect("number") - 1.3).abs() < 1e-6);
    assert_eq!(body["accentPhrases"][0]["moras"][0]["vowel"], "o");
    assert_eq!(body["kana"], "コ");
}

#[tokio::test]
async fn numeric_voice_overrides_configured_speaker() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();

    plugin
        .synthesize(
            KIND,
            config_json(&mock.url),
            "hello".to_string(),
            "42".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let requests = mock.requests.lock().expect("request log");
    assert!(requests[0].path.contains("speaker=42"));
}

#[tokio::test]
async fn external_mode_reports_engine_unreachable() {
    let port = pick_free_port();
    let plugin = test_plugin();

    let err = plugin
        .synthesize(
            KIND,
            config_json(&format!("http://127.0.0.1:{port}")),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("no engine is listening");

    assert!(err.to_string().contains("audio_query"));
}

#[tokio::test]
async fn rejects_non_wav_format() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();

    let err = plugin
        .synthesize(
            KIND,
            config_json(&mock.url),
            "hello".to_string(),
            String::new(),
            "mp3".to_string(),
        )
        .await
        .expect_err("mp3 is unsupported");

    assert!(err.to_string().contains("wav"));
}

#[tokio::test]
async fn managed_mode_uses_existing_server_without_spawning() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();

    let wav = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": mock.url,
                "auto_start": true,
                "engine_path": "/nonexistent/engine"
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("running server is reused without spawning");

    assert!(wav.starts_with(b"RIFF"));
}

#[tokio::test]
async fn managed_mode_requires_engine_path_when_server_down() {
    let port = pick_free_port();
    let plugin = test_plugin();

    let err = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": format!("http://127.0.0.1:{port}"),
                "auto_start": true
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("engine_path is missing");

    assert!(err.to_string().contains("engine_path"));
}

#[tokio::test]
async fn managed_mode_spawns_engine_and_kills_it_on_drop() {
    // Child branch: the plugin spawns this same test binary (filtered by
    // `engine_args` below) as the managed engine; the marker env var makes
    // it serve the mock engine instead of running assertions.
    if let Some(port) = fake_engine_child_port() {
        run_fake_engine_child(port).await;
    }
    let port = pick_free_port();
    {
        let _env_guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`; the
        // plugin spawns this test binary as the engine, and the marker makes
        // the child process serve the mock engine on this port.
        unsafe {
            std::env::set_var(FAKE_ENGINE_ENV, port.to_string());
        }
    }

    let plugin = test_plugin();
    let wav = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": format!("http://127.0.0.1:{port}"),
                "auto_start": true,
                "engine_path": std::env::current_exe().expect("test binary path"),
                "engine_args": ["managed_mode_spawns_engine_and_kills_it_on_drop"],
                "startup_timeout_secs": 15
            }),
            "こんにちは".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("managed synthesis succeeds");
    assert!(wav.starts_with(b"RIFF"));

    drop(plugin);

    // Dropping the plugin must kill the engine child; poll until the port
    // refuses connections.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let refused = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err();
        if refused {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "engine still serving after plugin drop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // SAFETY: test-only env cleanup, serialized by `ENV_MUTEX`.
    unsafe {
        std::env::remove_var(FAKE_ENGINE_ENV);
    }
}

/// Env var marking this process as the managed-mode fake engine child.
const FAKE_ENGINE_ENV: &str = "ENE_VOICEVOX_FAKE_ENGINE";

fn fake_engine_child_port() -> Option<u16> {
    std::env::var(FAKE_ENGINE_ENV).ok()?.parse().ok()
}

/// Serves the mock engine forever; only ever reached by the spawned child.
async fn run_fake_engine_child(port: u16) {
    let listener = bind_with_retries(port).await.expect("fake engine bind");
    crate::mock_engine::serve_engine(listener, Arc::new(StdMutex::new(Vec::new()))).await;
}

/// Binds the requested port, retrying briefly to absorb the race between
/// the parent dropping its probe listener and this process binding.
async fn bind_with_retries(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let mut last_error = std::io::Error::new(std::io::ErrorKind::AddrInUse, "port still in use");
    for _ in 0..20 {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) => last_error = e,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(last_error)
}

#[test]
fn config_schema_advertises_all_settings() {
    let schema = test_plugin().config_schema().expect("schema present");
    for key in [
        "server_url",
        "speaker_id",
        "speed_scale",
        "pitch_scale",
        "intonation_scale",
        "volume_scale",
        "tempo_dynamics_scale",
        "output_sampling_rate",
        "auto_start",
        "engine_path",
        "engine_args",
        "startup_timeout_secs",
    ] {
        assert!(
            schema["properties"][key].is_object(),
            "missing schema key {key}"
        );
    }
}

#[test]
fn tts_capabilities_declare_kind_formats_and_serial_concurrency() {
    let capabilities = test_plugin().tts_capabilities();
    assert_eq!(capabilities.len(), 1);
    assert_eq!(capabilities[0].kind, "voicevox");
    assert_eq!(capabilities[0].formats, vec!["wav"]);
    assert_eq!(capabilities[0].concurrency.max_in_flight, 1);
    assert_eq!(capabilities[0].concurrency.queue_depth, 2);
}

#[test]
fn empty_provider_config_parses_to_defaults() {
    let config = VoicevoxConfig::from_value(json!({})).expect("empty config parses");
    assert_eq!(config.server_url, "http://127.0.0.1:50021");
    assert_eq!(config.speaker_id, 0);
    assert!(!config.auto_start);
}
