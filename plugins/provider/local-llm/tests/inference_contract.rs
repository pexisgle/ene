//! CPU inference contract tests against the real plugin binary.
//!
//! Spawns `ene-plugin-llama-cpp` and drives it over the v6 `MessagePack` wire
//! protocol: chat streaming, JSON-schema completion, GGUF embeddings, and
//! typed error paths. The GGUF fixtures are downloaded from pinned URLs into
//! a blake3-keyed cache and verified; a download/transport failure skips the
//! tests so CI stays green without network access, while a hash mismatch
//! fails loudly because the pin itself drifted (harness in `common`).
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "integration tests use expect/panic for assertions and eprintln for skip diagnostics"
)]

mod common;
use common::*;

use ene_plugin_proto::{PluginIpcRequest, PluginIpcResponse, WireFormat, write_plugin_request};
use serde_json::json;

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
