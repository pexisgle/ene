//! CPU inference contract tests against the real plugin binary.
//!
//! Spawns `ene-plugin-llama-cpp` and drives it over the v6 `MessagePack` wire
//! protocol: chat streaming, JSON-schema completion, GGUF embeddings, and
//! typed error paths. The GGUF fixtures are downloaded from pinned URLs into
//! a blake3-keyed cache and verified; a download/transport failure skips the
//! tests so CI stays green without network access, while a hash mismatch
//! fails loudly because the pin itself drifted.
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "integration tests use expect/panic for assertions and eprintln for skip diagnostics"
)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ene_ai::model_fetch::{MagicBytesValidator, ModelFetcher};
use ene_plugin_proto::{
    IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginIpcRequest, PluginIpcResponse, SandboxConfigData,
    VersionRange, WireFormat, cleanup_path, read_plugin_response, write_plugin_request,
};
use serde_json::{Value, json};

/// llama.cpp's canonical tiny chat model (~1.2 MB, real tensors).
const CHAT_FIXTURE_URL: &str = concat!(
    "https://huggingface.co/ggml-org/models-moved/resolve/",
    "499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf"
);
const CHAT_FIXTURE_BLAKE3: &str =
    "1b2a49c52e28947a42c0b93606e1cd4554dc97f046f37fb292fbaa3517acdcc0";

/// A real BERT embedding model in GGUF (`n_embd` = 384).
const EMBED_FIXTURE_URL: &str = concat!(
    "https://huggingface.co/ggml-org/models-moved/resolve/",
    "499bc8821c6b12b4e53c5bffcb21ec206f212d81/bert-bge-small/ggml-model-f16.gguf"
);
const EMBED_FIXTURE_BLAKE3: &str =
    "57949329d93364735b75d171b995a5b3facad284c0d246f38f2803ff335ec79e";

/// GGUF magic validator shared by the fixture downloads.
static GGUF_VALIDATOR: MagicBytesValidator = MagicBytesValidator::new("gguf", b"GGUF");

static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Path to the plugin binary cargo built for this test package.
fn plugin_binary() -> &'static str {
    option_env!("CARGO_BIN_EXE_ene-plugin-llama-cpp")
        .or(option_env!("CARGO_BIN_EXE_ene_plugin_llama_cpp"))
        .expect("cargo sets CARGO_BIN_EXE_* for integration tests")
}

/// Hash-verified fixture paths from the blake3-keyed cache.
struct Fixtures {
    chat: PathBuf,
    embed: PathBuf,
}

impl Fixtures {
    /// Ensures both fixtures are cached and verified; `None` skips the tests
    /// when the network is unavailable. A pinned-hash mismatch panics — the
    /// pin must be updated, not silently skipped.
    async fn fetch() -> Option<Self> {
        let chat =
            match fetch_fixture("stories260K.gguf", CHAT_FIXTURE_URL, CHAT_FIXTURE_BLAKE3).await {
                Ok(path) => path,
                Err(FixtureError::Unavailable(message)) => {
                    eprintln!("skipping local-llm inference contract tests: {message}");
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
                eprintln!("skipping local-llm inference contract tests: {message}");
                return None;
            }
            Err(FixtureError::PinDrifted(message)) => {
                panic!("pinned fixture drifted; update the pin: {message}");
            }
        };
        Some(Self { chat, embed })
    }
}

/// Why a fixture is not usable.
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
    base.join("ene").join("local-llm-fixtures")
}

async fn fixture_hash_matches(path: &Path, expected_blake3: &str) -> bool {
    match tokio::fs::read(path).await {
        Ok(bytes) => blake3::hash(&bytes).to_hex().to_string() == expected_blake3,
        Err(_) => false,
    }
}

/// Kills the spawned plugin on drop so tests never leave orphan processes.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    /// SIGKILLs the plugin and reaps it, simulating an in-process crash.
    fn kill(&mut self) -> Option<std::process::ExitStatus> {
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

/// A spawned plugin process with a connected, handshaken IPC stream.
struct PluginSession {
    child: ChildGuard,
    stream: IpcStream,
    socket_path: PathBuf,
}

impl PluginSession {
    async fn start(profiles: &Value) -> Self {
        let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket_path = PathBuf::from(format!(
            "/tmp/ene-local-llm-contract-{}-{id}.sock",
            std::process::id()
        ));
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
                plugin_config: Some(json!({ "acceleration": "cpu" })),
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

    /// SIGKILLs the plugin process and reaps it, returning its exit status.
    fn kill_plugin(&mut self) -> Option<std::process::ExitStatus> {
        self.child.kill()
    }

    async fn request(&mut self, request: PluginIpcRequest) -> PluginIpcResponse {
        write_plugin_request(&mut self.stream, &request, WireFormat::MsgPack)
            .await
            .expect("write request");
        read_plugin_response(&mut self.stream, WireFormat::MsgPack)
            .await
            .expect("read response")
            .expect("response frame")
    }

    async fn read_response(&mut self) -> PluginIpcResponse {
        read_plugin_response(&mut self.stream, WireFormat::MsgPack)
            .await
            .expect("read stream response")
            .expect("stream response frame")
    }

    /// Drains a chat stream until `StreamEnd`, returning (text deltas seen,
    /// full text, final chunk as (text-delta-empty, usage-present)).
    async fn drain_chat_stream(&mut self, request_id: &str) -> (usize, String, (bool, bool)) {
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

fn user_message(text: &str) -> Value {
    json!({
        "role": "user",
        "parts": [{ "Text": { "text": text } }]
    })
}

/// The full CPU inference contract: streaming, completion, embeddings, and
/// typed failures on a live plugin process.
#[tokio::test]
async fn inference_contract_round_trip() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let profiles = json!({
        "chat-fixture": {
            "model_path": fixtures.chat.to_str().expect("utf8 fixture path"),
            "quantization": "F16",
            "gpu_layers": "0",
            "context_size": 2048,
        },
        "embed-fixture": {
            "model_path": fixtures.embed.to_str().expect("utf8 fixture path"),
            "quantization": "F16",
            "gpu_layers": "0",
        }
    });
    let mut session = PluginSession::start(&profiles).await;

    // 1. Real token-by-token streaming: more than one text delta and usage
    // on the final chunk.
    write_plugin_request(
        &mut session.stream,
        &PluginIpcRequest::CreateChatStream {
            request_id: "req-stream".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("Once upon a time")],
            tools: Vec::new(),
        },
        WireFormat::MsgPack,
    )
    .await
    .expect("write create_chat_stream");
    let (deltas, full_text, final_chunk) = session.drain_chat_stream("req-stream").await;
    assert!(
        deltas >= 2,
        "expected real token-by-token streaming, got {deltas} text deltas (text: {full_text:?})"
    );
    let (final_text_empty, final_usage) = final_chunk;
    assert!(
        final_text_empty && final_usage,
        "the final stream chunk must be a usage-only chunk (empty text delta, usage present)"
    );
    assert!(
        !full_text.trim().is_empty(),
        "expected non-empty streamed text"
    );

    // 2. Non-streaming completion constrained by a JSON schema (grammar path).
    let completion = session
        .request(PluginIpcRequest::ChatCompletion {
            request_id: "req-completion".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("Reply with JSON only: {\"ok\": true}")],
            json_schema: Some(json!({
                "type": "object",
                "properties": { "ok": { "type": "boolean" } },
                "required": ["ok"],
                "additionalProperties": false
            })),
        })
        .await;
    let PluginIpcResponse::ChatCompletionResult { content, usage, .. } = completion else {
        panic!("expected ChatCompletionResult, got {completion:?}");
    };
    assert!(
        !content.trim().is_empty(),
        "expected non-empty grammar completion"
    );
    assert!(usage.is_some(), "local completion reports token usage");

    // 3. GGUF embeddings: one vector per item, model dims, finite values.
    let embedded = session
        .request(PluginIpcRequest::EmbedBatch {
            request_id: "req-embed".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "embed-fixture".to_string(),
            dimensions: None,
            items: vec![
                "The cat sat on the mat.".to_string(),
                "A dog barked at the moon.".to_string(),
            ],
        })
        .await;
    let PluginIpcResponse::EmbedBatchResult { embeddings, .. } = embedded else {
        panic!("expected EmbedBatchResult, got {embedded:?}");
    };
    assert_eq!(embeddings.len(), 2, "one vector per input item");
    for vector in &embeddings {
        assert_eq!(vector.len(), 384, "bge-small produces 384 dims");
        assert!(
            vector.iter().all(|value| value.is_finite()),
            "embedding values must be finite"
        );
    }

    // 4. Typed failures, then a successful request proves the process
    // survived each one (no panics, no crashes).
    let unknown = session
        .request(PluginIpcRequest::CreateChatStream {
            request_id: "req-unknown".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "no-such-model".to_string(),
            max_tokens: None,
            messages: vec![user_message("hi")],
            tools: Vec::new(),
        })
        .await;
    let PluginIpcResponse::StreamError { message, .. } = unknown else {
        panic!("expected StreamError for unknown model, got {unknown:?}");
    };
    assert!(
        message.contains("profile"),
        "missing profile error: {message}"
    );

    let tools = session
        .request(PluginIpcRequest::CreateChatStream {
            request_id: "req-tools".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("hi")],
            tools: vec![json!({
                "name": "ns.tool",
                "description": "a tool",
                "parameters": {}
            })],
        })
        .await;
    let PluginIpcResponse::StreamError { message, .. } = tools else {
        panic!("expected StreamError for tools, got {tools:?}");
    };
    assert!(
        message.contains("tool"),
        "tools rejection must mention tool calls: {message}"
    );

    let bad_dims = session
        .request(PluginIpcRequest::EmbedBatch {
            request_id: "req-dims".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "embed-fixture".to_string(),
            dimensions: Some(64),
            items: vec!["text".to_string()],
        })
        .await;
    let PluginIpcResponse::Error { message, .. } = bad_dims else {
        panic!("expected Error for dimension mismatch, got {bad_dims:?}");
    };
    assert!(message.contains("dims"), "dimension error: {message}");

    let alive = session
        .request(PluginIpcRequest::ChatCompletion {
            request_id: "req-alive".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("Say hello.")],
            json_schema: None,
        })
        .await;
    let PluginIpcResponse::ChatCompletionResult { content, .. } = alive else {
        panic!("plugin died after typed errors; got {alive:?}");
    };
    assert!(
        !content.trim().is_empty(),
        "expected a completion after error paths"
    );

    let socket_path = session.socket_path.clone();
    drop(session);
    cleanup_path(&socket_path);
}

/// A real plugin process dying (SIGKILL, as an in-process llama.cpp abort
/// terminates it) must not take the host down, and a freshly restarted plugin
/// must serve real inference again.
#[tokio::test]
async fn plugin_crash_isolation_recovers() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let profiles = json!({
        "chat-fixture": {
            "model_path": fixtures.chat.to_str().expect("utf8 fixture path"),
            "quantization": "F16",
            "gpu_layers": "0",
            "context_size": 2048,
        }
    });

    let mut first = PluginSession::start(&profiles).await;
    write_plugin_request(
        &mut first.stream,
        &PluginIpcRequest::CreateChatStream {
            request_id: "crash-before".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("Once upon a time")],
            tools: Vec::new(),
        },
        WireFormat::MsgPack,
    )
    .await
    .expect("write create_chat_stream before crash");
    let (deltas, full_text, _) = first.drain_chat_stream("crash-before").await;
    assert!(
        deltas >= 1 && !full_text.is_empty(),
        "expected real streaming before the crash, got {deltas} deltas ({full_text:?})"
    );

    // Simulate an in-process llama.cpp abort: SIGKILL the plugin and reap it.
    let status = first
        .kill_plugin()
        .expect("killed plugin reports an exit status");
    let first_socket = first.socket_path.clone();
    drop(first);
    cleanup_path(&first_socket);
    assert!(
        !status.success(),
        "plugin process must not exit successfully after SIGKILL"
    );
    #[cfg(unix)]
    assert!(
        status.code().is_none(),
        "SIGKILL must terminate via a signal, got exit code {:?}",
        status.code()
    );

    // The host side (this test process) is still alive and a restarted
    // plugin serves real inference again.
    let mut second = PluginSession::start(&profiles).await;
    write_plugin_request(
        &mut second.stream,
        &PluginIpcRequest::CreateChatStream {
            request_id: "crash-after".to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message("Once upon a time")],
            tools: Vec::new(),
        },
        WireFormat::MsgPack,
    )
    .await
    .expect("write create_chat_stream after restart");
    let (deltas, full_text, _) = second.drain_chat_stream("crash-after").await;
    assert!(
        deltas >= 1 && !full_text.is_empty(),
        "expected real streaming after restart, got {deltas} deltas ({full_text:?})"
    );

    let socket_path = second.socket_path.clone();
    drop(second);
    cleanup_path(&socket_path);
}
