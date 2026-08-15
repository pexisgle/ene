//! Plugin tests: external-mode 2-step flow against an in-process fake
//! engine, error paths, and managed-mode lifecycle against a real engine
//! child process (the test harness re-executes itself, filtered to this
//! test, and serves the mock engine under an env-var marker).

#![expect(
    clippy::expect_used,
    reason = "unit tests use expect/expect_err for concise assertions"
)]

use std::ffi::{OsStr, OsString};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use ene_plugin::{ConfigurablePlugin as _, TtsPlugin as _};
use serde_json::{Value, json};

use crate::config::VoicevoxConfig;
use crate::mock_engine::spawn_mock_engine;
use crate::plugin::VoicevoxPlugin;

static ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct ScopedEnv {
    key: &'static str,
    previous: Option<OsString>,
    _lock: tokio::sync::MutexGuard<'static, ()>,
}

impl ScopedEnv {
    fn set(
        lock: tokio::sync::MutexGuard<'static, ()>,
        key: &'static str,
        value: impl AsRef<OsStr>,
    ) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every environment access in this test binary is serialized
        // by `ENV_MUTEX`, which remains held until this guard restores it.
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: `_lock` is still held while Drop restores the process-wide
        // value, including when a test unwinds after a failed assertion.
        unsafe {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

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
async fn speaker_options_query_running_engine() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();
    plugin.set_config(&json!({"server_url": mock.url}));

    let options = plugin.list_config_options("speakers").await;
    assert_eq!(options.len(), 3);
    assert_eq!(options[0].value, json!(2));
    assert_eq!(options[0].label, "四国めたん / ノーマル");
    assert_eq!(options[0].group.as_deref(), Some("四国めたん"));
    assert_eq!(options[1].value, json!(0));
    assert_eq!(options[2].value, json!(3));
}

#[tokio::test]
async fn speaker_options_are_empty_when_engine_is_down() {
    let port = pick_free_port();
    let plugin = test_plugin();
    plugin.set_config(&json!({
        "server_url": format!("http://127.0.0.1:{port}"),
        "mode": "managed",
        "server_path": "/nonexistent/engine"
    }));

    // Listing speakers must never spawn a managed engine or block forever;
    // a down engine simply yields no candidates.
    let options = plugin.list_config_options("speakers").await;
    assert!(options.is_empty());
    assert!(plugin.list_config_options("voices").await.is_empty());
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
                "mode": "managed",
                "server_path": "/nonexistent/engine"
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
                "mode": "managed"
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("engine_path is missing");

    assert!(err.to_string().contains("server_path"));
}

#[tokio::test]
async fn managed_mode_reports_spawn_failure() {
    let port = pick_free_port();
    let plugin = test_plugin();

    let err = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": format!("http://127.0.0.1:{port}"),
                "mode": "managed",
                "server_path": "/nonexistent/ene-engine-binary",
                "startup_timeout_secs": 1
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("spawn of a missing binary fails");

    assert!(err.to_string().contains("failed to spawn VOICEVOX engine"));
}

#[tokio::test]
async fn managed_mode_reports_startup_timeout() {
    let port = pick_free_port();
    let plugin = test_plugin();

    // `sleep` spawns fine but never answers GET /version; the startup
    // timeout must fire and kill it (minimum timeout is 1 s).
    let err = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": format!("http://127.0.0.1:{port}"),
                "mode": "managed",
                "server_path": "sleep",
                "server_args": ["30"],
                "startup_timeout_secs": 1
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("engine never answers /version");

    assert!(err.to_string().contains("did not answer GET /version"));
}

#[tokio::test]
async fn managed_mode_spawns_engine_and_kills_it_on_drop() {
    let env_lock = ENV_MUTEX.lock().await;
    // Child branch: the plugin spawns this same test binary (filtered with
    // `--exact` by `server_args` below) as the managed engine; the marker
    // env var makes the child serve the mock engine instead of running
    // assertions.
    if let Some(port) = fake_engine_child_port() {
        run_fake_engine_child(port).await;
    }
    let port = pick_free_port();
    let _env_guard = ScopedEnv::set(env_lock, FAKE_ENGINE_ENV, port.to_string());

    let plugin = test_plugin();
    let wav = plugin
        .synthesize(
            KIND,
            json!({
                "server_url": format!("http://127.0.0.1:{port}"),
                "mode": "managed",
                "server_path": std::env::current_exe().expect("test binary path"),
                "server_args": [
                    "--exact",
                    "tests::managed_mode_spawns_engine_and_kills_it_on_drop"
                ],
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
}

/// The delivered `set_config` blob is canonical: a stale request blob (raw
/// persisted config without the injected `server_path`) must not win over
/// the config the host delivered at handshake / live `SetConfig`.
#[tokio::test]
async fn synthesis_uses_delivered_config_over_stale_request_blob() {
    let mock = spawn_mock_engine().expect("mock engine");
    let plugin = test_plugin();
    plugin.set_config(&json!({
        "server_url": mock.url,
        "mode": "managed",
        "server_path": "/nonexistent/engine"
    }));

    let wav = plugin
        .synthesize(
            KIND,
            // A stale request blob: points at a dead port and omits
            // server_path entirely. The delivered config must win.
            json!({
                "server_url": format!("http://127.0.0.1:{}", pick_free_port()),
                "mode": "managed"
            }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("delivered config drives synthesis");
    assert!(wav.starts_with(b"RIFF"));
}

/// Changing the launch signature via `set_config` stops the old engine
/// child; the next synthesis starts one with the new settings.
#[tokio::test]
async fn set_config_stops_engine_when_launch_signature_changes() {
    let env_lock = ENV_MUTEX.lock().await;
    if let Some(port) = fake_engine_child_port() {
        run_fake_engine_child(port).await;
    }
    let port = pick_free_port();
    let _env_guard = ScopedEnv::set(env_lock, FAKE_ENGINE_ENV, port.to_string());
    let executable = std::env::current_exe().expect("test binary path");
    let plugin = test_plugin();
    let server_url = format!("http://127.0.0.1:{port}");

    plugin.set_config(&json!({
        "server_url": server_url,
        "mode": "managed",
        "server_path": executable,
        "server_args": ["--exact", "tests::set_config_stops_engine_when_launch_signature_changes"],
        "startup_timeout_secs": 15
    }));
    plugin
        .synthesize(
            KIND,
            json!({"server_url": server_url, "mode": "managed"}),
            "こんにちは".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("first launch starts the engine");

    // A different launch signature must stop the child started above.
    plugin.set_config(&json!({
        "server_url": server_url,
        "mode": "managed",
        "server_path": executable,
        "server_args": [
            "--exact",
            "tests::set_config_stops_engine_when_launch_signature_changes",
            "--nocapture"
        ],
        "startup_timeout_secs": 15
    }));

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
            "old engine still serving after launch signature change"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The next synthesis must start a fresh engine from the new launch
    // signature (the spawned kill task ran asynchronously). The marker env
    // stays set so the spawned child serves the mock engine again.
    let second_wav = plugin
        .synthesize(
            KIND,
            json!({"server_url": server_url, "mode": "managed"}),
            "またね".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("second launch uses the new signature");
    assert!(second_wav.starts_with(b"RIFF"));

    drop(plugin);
}

/// End-to-end wire contract: the config delivered at handshake (with an
/// artifact-injected `server_path`) is canonical. A stale request blob —
/// the raw persisted config, pointing at a dead port and omitting
/// `server_path` — must not override it on the synthesize path.
#[tokio::test]
async fn ipc_handshake_delivered_config_beats_stale_request_blob() {
    use ene_plugin::SandboxConfigData;
    use ene_plugin_host::IpcPluginConnection;
    use std::time::Duration;

    let mock = spawn_mock_engine().expect("mock engine");
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("plugin.sock");
    let env_lock = ENV_MUTEX.lock().await;
    let _env_guard = ScopedEnv::set(env_lock, "ENE_PLUGIN_SOCKET", &socket);

    let dispatch = ene_plugin::PluginDispatch::new(
        None,
        None,
        None,
        Some(std::sync::Arc::new(VoicevoxPlugin::default())),
        None,
    );
    let server = tokio::spawn(async move {
        drop(ene_plugin::run_plugin_server(dispatch).await);
    });
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let delivered = json!({
        "server_url": mock.url,
        "mode": "managed",
        "server_path": "/nonexistent/engine"
    });
    let conn = IpcPluginConnection::connect(
        &socket,
        SandboxConfigData::default(),
        Some(delivered),
        None,
        Duration::from_secs(5),
        4,
    )
    .await
    .expect("handshake with delivered config");

    let dead_url = format!("http://127.0.0.1:{}", pick_free_port());
    let (audio_base64, _) = conn
        .synthesize_speech(
            String::new(),
            KIND.to_string(),
            json!({ "server_url": dead_url, "mode": "managed" }),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("delivered config drives synthesis over the stale blob");
    assert!(
        !audio_base64.is_empty(),
        "synthesis against the delivered engine endpoint must succeed"
    );

    drop(conn);
    server.abort();
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
        "mode",
        "server_path",
        "server_args",
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
    assert_eq!(config.mode(), crate::config::EngineMode::External);
}
