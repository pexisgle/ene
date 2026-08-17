use crate::{
    CORE_VERSION, HostConn, HostHello, IpcError, ProtoId, ProtocolRanges, ToolCall, ToolHandler,
    ToolSpecWire, VersionRange, serve_plugin,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::net::UnixStream;

struct EchoHandler;

#[async_trait]
#[expect(
    clippy::unnecessary_literal_bound,
    reason = "test stand-in returns fixed plugin identity strings"
)]
impl ToolHandler for EchoHandler {
    fn plugin_id(&self) -> &str {
        "tool.utility"
    }
    fn plugin_name(&self) -> &str {
        "utility"
    }
    fn digest(&self) -> &str {
        "sha256:test"
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        vec![ToolSpecWire {
            name: "utility.hash".to_owned(),
            description: "hash text".to_owned(),
            parameters: json!({"type":"object"}),
            output: json!({"type":"object"}),
            side_effects: Vec::new(),
        }]
    }
    async fn call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        if name != "utility.hash" {
            return Err(IpcError::UnknownTool(name.to_owned()));
        }
        let text = args
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(json!({ "sha256": format!("{:x}", md5_stub(text)) }))
    }
    fn spawn_token(&self) -> Result<String, String> {
        Ok(SPAWN_TOKEN.to_owned())
    }
}

fn md5_stub(text: &str) -> u64 {
    let mut h = 0_u64;
    for byte in text.as_bytes() {
        h = h.wrapping_mul(31).wrapping_add(u64::from(*byte));
    }
    h
}

const SPAWN_TOKEN: &str = "test-spawn-token";

fn hello() -> HostHello {
    HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool],
    }
}

fn spawn_echo_plugin(plugin_side: UnixStream) -> tokio::task::JoinHandle<Result<(), IpcError>> {
    tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await })
}

#[tokio::test]
async fn ping_and_tool_call_roundtrip() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = spawn_echo_plugin(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    host.ping().await.unwrap();
    let specs = host.list_tools().await.unwrap();
    assert_eq!(specs[0].name, "utility.hash");
    assert!(specs[0].side_effects.is_empty());
    let result = host
        .call_tool(ToolCall {
            call_id: "c1".to_owned(),
            tool_name: "utility.hash".to_owned(),
            args: json!({"text": "hi"}),
            deadline_ms: None,
        })
        .await
        .unwrap();
    assert_eq!(result.status, "ok");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn tool_face_disabled_when_manifest_omits_it() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = spawn_echo_plugin(plugin_side);
    let mut hello = hello();
    hello.declared_protocols = vec![ProtoId::Core];
    let mut host = HostConn::handshake(host_side, hello, &[ProtoId::Core], SPAWN_TOKEN)
        .await
        .unwrap();
    assert!(host.negotiated().tool.is_none());
    assert!(host.list_tools().await.unwrap().is_empty());
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn core_range_mismatch_rejects() {
    let mut ranges = ProtocolRanges::host_supported();
    ranges.core = VersionRange {
        min: CORE_VERSION + 1,
        max: CORE_VERSION + 1,
    };
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ranges,
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool],
    };
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = spawn_echo_plugin(plugin_side);
    let err = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        IpcError::Rejected(_) | IpcError::CoreIncompatible
    ));
    drop(plugin);
}

#[tokio::test]
async fn spawn_token_mismatch_rejects() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = spawn_echo_plugin(plugin_side);
    let err = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        "wrong-token",
    )
    .await
    .unwrap_err();
    assert!(matches!(err, IpcError::DigestMismatch));
    drop(plugin);
}

fn record_baseline(name: &str, body: impl AsRef<[u8]>) {
    std::fs::create_dir_all("/opt/cursor/artifacts").unwrap();
    std::fs::write(format!("/opt/cursor/artifacts/{name}"), body).unwrap();
}

#[tokio::test]
async fn ipc_baseline_ping_is_measurable() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = spawn_echo_plugin(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    host.ping().await.unwrap();
    const N: u32 = 50;
    let ping_started = std::time::Instant::now();
    for _ in 0..N {
        host.ping().await.unwrap();
    }
    let ping_mean_us = ping_started.elapsed().as_micros() / u128::from(N);

    let call = ToolCall {
        call_id: "bench".to_owned(),
        tool_name: "utility.hash".to_owned(),
        args: json!({"text": "hi"}),
        deadline_ms: None,
    };
    host.call_tool(call.clone()).await.unwrap();
    let tool_started = std::time::Instant::now();
    for i in 0..N {
        let mut call = call.clone();
        call.call_id = format!("bench-{i}");
        host.call_tool(call).await.unwrap();
    }
    let tool_mean_us = tool_started.elapsed().as_micros() / u128::from(N);

    record_baseline(
        "ipc_baseline.txt",
        format!("ping_roundtrip_mean_us={ping_mean_us} tool_call_mean_us={tool_mean_us} n={N}\n"),
    );
    assert!(
        ping_mean_us < 5_000,
        "ipc ping regression ping_mean_us={ping_mean_us}"
    );
    assert!(
        tool_mean_us < 5_000,
        "ipc tool-call regression tool_mean_us={tool_mean_us}"
    );
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}
