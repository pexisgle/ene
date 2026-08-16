//! Shared sidecar-contract-test harness: pinned GGUF fixtures, a resolved
//! `llama-server` binary, and a spawned plugin process with a handshaken IPC
//! stream.
//!
//! The fixtures are downloaded from pinned URLs into a blake3-keyed cache
//! and verified; a download/transport failure skips the tests so CI stays
//! green without network access, while a hash mismatch fails loudly because
//! the pin itself drifted. Tests also skip when no `llama-server` binary can
//! be resolved.
#![expect(
    dead_code,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "shared harness: each test target uses only a subset of it"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ene_ai::model_fetch::{MagicBytesValidator, ModelFetcher};
use ene_plugin_proto::{
    IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginIpcRequest, PluginIpcResponse, SandboxConfigData,
    VersionRange, WireFormat, read_plugin_response, write_plugin_request,
};
use serde_json::{Value, json};

pub use ene_plugin_proto::cleanup_path;

/// llama.cpp's canonical tiny chat model (~1.2 MB, real tensors).
pub const CHAT_FIXTURE_URL: &str = concat!(
    "https://huggingface.co/ggml-org/models-moved/resolve/",
    "499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf"
);
pub const CHAT_FIXTURE_BLAKE3: &str =
    "1b2a49c52e28947a42c0b93606e1cd4554dc97f046f37fb292fbaa3517acdcc0";

/// A real BERT embedding model in GGUF (`n_embd` = 384).
pub const EMBED_FIXTURE_URL: &str = concat!(
    "https://huggingface.co/ggml-org/models-moved/resolve/",
    "499bc8821c6b12b4e53c5bffcb21ec206f212d81/bert-bge-small/ggml-model-f16.gguf"
);
pub const EMBED_FIXTURE_BLAKE3: &str =
    "57949329d93364735b75d171b995a5b3facad284c0d246f38f2803ff335ec79e";

static GGUF_VALIDATOR: MagicBytesValidator = MagicBytesValidator::new("gguf", b"GGUF");

static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn plugin_binary() -> &'static str {
    option_env!("CARGO_BIN_EXE_ene-plugin-llama-server")
        .or(option_env!("CARGO_BIN_EXE_ene_plugin_llama_server"))
        .expect("cargo sets CARGO_BIN_EXE_* for integration tests")
}

/// Resolves a usable `llama-server` binary: `ENE_LLAMA_SERVER_BIN`, the
/// plugin's own directory, then `PATH`. `None` skips the tests.
pub fn llama_server_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ENE_LLAMA_SERVER_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let bundled = ene_config::builtin_plugins_dir().join(sidecar_name());
    if bundled.is_file() {
        return Some(bundled);
    }
    let probe = Command::new("sh")
        .args(["-c", "command -v llama-server"])
        .output()
        .ok()?;
    if probe.status.success() {
        let path = String::from_utf8_lossy(&probe.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

fn sidecar_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Handshake config pointing at the resolved sidecar binary, CPU-only.
pub fn sidecar_config() -> Option<Value> {
    Some(json!({
        "server_path": llama_server_binary()?,
        "acceleration": "cpu",
        "startup_timeout_secs": 60,
    }))
}

/// Unique short socket path (SUN_LEN-safe under long `TMPDIR` values).
pub fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ene-lm-{}-{id}-{name}.sock", std::process::id()))
}

/// Hash-verified fixture paths from the blake3-keyed cache.
pub struct Fixtures {
    pub chat: PathBuf,
    pub embed: PathBuf,
}

impl Fixtures {
    /// Ensures both fixtures are cached and verified; `None` skips the tests
    /// when the network is unavailable. A pinned-hash mismatch panics — the
    /// pin must be updated, not silently skipped.
    pub async fn fetch() -> Option<Self> {
        let chat =
            match fetch_fixture("stories260K.gguf", CHAT_FIXTURE_URL, CHAT_FIXTURE_BLAKE3).await {
                Ok(path) => path,
                Err(FixtureError::Unavailable(message)) => {
                    eprintln!("skipping llama-server inference contract tests: {message}");
                    return None;
                }
                Err(FixtureError::PinDrifted(message)) => {
                    panic!("pinned fixture drifted; update the pin: {message}");
                }
            };
        let embed = match fetch_fixture(
            "bge-small-f16.gguf",
            EMBED_FIXTURE_URL,
            EMBED_FIXTURE_BLAKE3,
        )
        .await
        {
            Ok(path) => path,
            Err(FixtureError::Unavailable(message)) => {
                eprintln!("skipping llama-server inference contract tests: {message}");
                return None;
            }
            Err(FixtureError::PinDrifted(message)) => {
                panic!("pinned fixture drifted; update the pin: {message}");
            }
        };
        Some(Self { chat, embed })
    }
}

enum FixtureError {
    /// Transport-level failure (network down, CDN issue): skip the tests.
    Unavailable(String),
    /// The downloaded bytes differ from the pinned blake3: fail the tests.
    PinDrifted(String),
}

/// Ensures `url` is cached under the pinned blake3, validating GGUF magic.
///
/// The cache lives under the user cache dir, keyed by the pinned hash so
/// re-runs reuse a verified download instead of re-fetching the 67 MB
/// embedding fixture (`target/tmp` is wiped by cargo between builds, so the
/// per-target temp dir would defeat the cache).
async fn fetch_fixture(
    name: &str,
    url: &str,
    expected_blake3: &str,
) -> Result<PathBuf, FixtureError> {
    let cache = fixture_cache_dir();
    let dest = cache.join(format!("{expected_blake3}-{name}"));
    if fixture_hash_matches(&dest, expected_blake3).await {
        return Ok(dest);
    }
    ModelFetcher::new()
        .fetch(url, &dest, &GGUF_VALIDATOR)
        .await
        .map_err(|e| FixtureError::Unavailable(format!("fixture download failed: {e}")))?;
    let bytes = tokio::fs::read(&dest)
        .await
        .map_err(|e| FixtureError::Unavailable(format!("fixture read failed: {e}")))?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    if digest != expected_blake3 {
        return Err(FixtureError::PinDrifted(format!(
            "fixture hash mismatch: expected {expected_blake3}, got {digest}"
        )));
    }
    Ok(dest)
}

/// Stable cache dir for verified fixtures (XDG cache, falling back to the
/// home cache). Files are content-addressed by the pinned blake3 and
/// re-verified on every use, so a stale or tampered entry is re-downloaded
/// rather than trusted.
fn fixture_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("ene").join("llama-server-fixtures")
}

async fn fixture_hash_matches(path: &Path, expected_blake3: &str) -> bool {
    match tokio::fs::read(path).await {
        Ok(bytes) => blake3::hash(&bytes).to_hex().to_string() == expected_blake3,
        Err(_) => false,
    }
}

/// Kills the spawned plugin on drop so tests never leave orphan processes.
pub struct ChildGuard(Option<Child>);

impl ChildGuard {
    #[must_use]
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }

    /// Simulates an in-process crash.
    pub fn kill(&mut self) -> Option<std::process::ExitStatus> {
        let mut child = self.0.take()?;
        drop(child.kill());
        child.wait().ok()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

pub struct PluginSession {
    pub child: ChildGuard,
    pub stream: IpcStream,
    pub socket_path: PathBuf,
}

impl PluginSession {
    pub async fn start(profiles: &Value, config: &Value) -> Self {
        let socket_path = test_socket_path("session");
        cleanup_path(&socket_path);
        let child = Command::new(plugin_binary())
            .env("ENE_PLUGIN_SOCKET", &socket_path)
            .spawn()
            .expect("spawn plugin binary");

        let mut stream = None;
        for _ in 0..50 {
            if let Ok(connected) = IpcStream::connect(&socket_path).await {
                stream = Some(connected);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut stream = stream.expect("plugin binary did not bind the socket");
        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange {
                    min: PLUGIN_IPC_PROTOCOL_VERSION,
                    max: PLUGIN_IPC_PROTOCOL_VERSION,
                },
                sandbox: SandboxConfigData::default(),
                plugin_config: Some(config.clone()),
                plugin_profiles: Some(profiles.clone()),
            },
            WireFormat::Json,
        )
        .await
        .expect("write handshake");
        let ack = read_plugin_response(&mut stream, WireFormat::Json)
            .await
            .expect("read handshake ack")
            .expect("handshake ack frame");
        assert!(
            matches!(ack, PluginIpcResponse::HandshakeAck { .. }),
            "expected HandshakeAck, got {ack:?}"
        );
        Self {
            child: ChildGuard(Some(child)),
            stream,
            socket_path,
        }
    }

    pub fn kill_plugin(&mut self) -> Option<std::process::ExitStatus> {
        self.child.kill()
    }

    pub async fn request(&mut self, request: PluginIpcRequest) -> PluginIpcResponse {
        write_plugin_request(&mut self.stream, &request, WireFormat::MsgPack)
            .await
            .expect("write request");
        read_plugin_response(&mut self.stream, WireFormat::MsgPack)
            .await
            .expect("read response")
            .expect("response frame")
    }

    pub async fn read_response(&mut self) -> PluginIpcResponse {
        read_plugin_response(&mut self.stream, WireFormat::MsgPack)
            .await
            .expect("read stream response")
            .expect("stream response frame")
    }

    /// Drains a chat stream until `StreamEnd`, returning (text deltas seen,
    /// full text, final chunk as (text-delta-empty, usage-present)).
    pub async fn drain_chat_stream(&mut self, request_id: &str) -> (usize, String, (bool, bool)) {
        let deadline = tokio::time::Instant::now() + Duration::from_mins(2);
        let mut deltas = 0_usize;
        let mut full_text = String::new();
        let mut final_chunk = (false, false);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "draining chat stream {request_id:?} exceeded the 2 min deadline"
            );
            let response = tokio::time::timeout(remaining, self.read_response())
                .await
                .expect("drain deadline exceeded while waiting for a stream frame");
            match response {
                PluginIpcResponse::StreamChunk {
                    request_id: chunk_rid,
                    text_delta,
                    usage,
                    ..
                } if chunk_rid == request_id => {
                    if !text_delta.is_empty() {
                        deltas += 1;
                        full_text.push_str(&text_delta);
                    }
                    final_chunk = (text_delta.is_empty(), usage.is_some());
                }
                PluginIpcResponse::StreamEnd {
                    request_id: end_rid,
                } if end_rid == request_id => break,
                other => panic!("unexpected stream response: {other:?}"),
            }
        }
        (deltas, full_text, final_chunk)
    }
}

/// The wire shape of a user message the host sends over IPC.
pub fn user_message(text: &str) -> Value {
    json!({
        "role": "user",
        "parts": [{ "Text": { "text": text } }]
    })
}
