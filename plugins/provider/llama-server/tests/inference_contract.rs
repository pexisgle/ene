//! End-to-end contract tests against the real plugin binary and a real
//! `llama-server` sidecar.
//!
//! The tests skip (not fail) when the network is unavailable or no
//! `llama-server` binary can be resolved, so CI stays green without either.
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "integration tests use expect/panic for assertions and eprintln for skip diagnostics"
)]

mod common;

use common::*;
use ene_plugin_proto::{PluginIpcRequest, PluginIpcResponse};
use serde_json::json;

#[tokio::test]
async fn inference_contract_round_trip_and_sidecar_recovery() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let Some(config) = sidecar_config() else {
        eprintln!("skipping llama-server inference contract tests: llama-server binary not found");
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
            "dimensions": 384,
        }
    });
    let mut session = PluginSession::start(&profiles, &config).await;

    // 1. Real token-by-token streaming: more than one text delta and usage
    // on the final chunk.
    write_stream(&mut session, "req-stream", "Once upon a time").await;
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

    // 2. Non-streaming completion constrained by a JSON schema.
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
        "expected non-empty schema completion"
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

    // 5. The plugin (and its sidecar) survived every typed error.
    write_stream(&mut session, "req-alive", "Say hello.").await;
    let (_, alive_text, _) = session.drain_chat_stream("req-alive").await;
    assert!(
        !alive_text.trim().is_empty(),
        "expected a stream after error paths"
    );

    // 6. A killed sidecar is respawned on the next request (Linux-only: the
    // test locates the sidecar by its unique preset directory name).
    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("pgrep")
            .args(["-f", "ene-llama-server"])
            .output()
            .expect("pgrep runs");
        let pids: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if pids.is_empty() {
            eprintln!("skipping sidecar crash recovery: no llama-server pid found");
        } else {
            eprintln!("killing sidecar pids: {pids:?}");
            let killed = std::process::Command::new("kill")
                .arg("-9")
                .args(&pids)
                .status()
                .expect("kill runs");
            assert!(killed.success(), "kill -9 sidecar failed");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            write_stream(&mut session, "req-recovered", "Are you still there?").await;
            let (_, recovered_text, _) = session.drain_chat_stream("req-recovered").await;
            assert!(
                !recovered_text.trim().is_empty(),
                "expected a stream after sidecar crash recovery"
            );
        }
    }

    let socket_path = session.socket_path.clone();
    drop(session);
    cleanup_path(&socket_path);
}

async fn write_stream(session: &mut PluginSession, request_id: &str, prompt: &str) {
    use ene_plugin_proto::WireFormat;
    use ene_plugin_proto::write_plugin_request;
    write_plugin_request(
        &mut session.stream,
        &PluginIpcRequest::CreateChatStream {
            request_id: request_id.to_string(),
            provider_kind: "local".to_string(),
            provider_config: json!({}),
            model: "chat-fixture".to_string(),
            max_tokens: None,
            messages: vec![user_message(prompt)],
            tools: Vec::new(),
        },
        WireFormat::MsgPack,
    )
    .await
    .expect("write create_chat_stream");
}
