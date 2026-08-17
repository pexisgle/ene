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
}

fn md5_stub(text: &str) -> u64 {
    let mut h = 0_u64;
    for byte in text.as_bytes() {
        h = h.wrapping_mul(31).wrapping_add(u64::from(*byte));
    }
    h
}

fn hello() -> HostHello {
    HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool],
    }
}

#[tokio::test]
async fn ping_and_tool_call_roundtrip() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await });
    let mut host = HostConn::handshake(host_side, hello(), &[ProtoId::Core, ProtoId::Tool])
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
    let plugin = tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await });
    let mut hello = hello();
    hello.declared_protocols = vec![ProtoId::Core];
    let mut host = HostConn::handshake(host_side, hello, &[ProtoId::Core])
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
    let plugin = tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await });
    let err = HostConn::handshake(host_side, hello, &[ProtoId::Core, ProtoId::Tool])
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        IpcError::Rejected(_) | IpcError::CoreIncompatible
    ));
    drop(plugin);
}

#[tokio::test]
async fn ipc_baseline_ping_is_measurable() {
    let (host_side, plugin_side) = UnixStream::pair().unwrap();
    let plugin = tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await });
    let mut host = HostConn::handshake(host_side, hello(), &[ProtoId::Core, ProtoId::Tool])
        .await
        .unwrap();
    let started = std::time::Instant::now();
    const N: u32 = 50;
    for _ in 0..N {
        host.ping().await.unwrap();
    }
    let elapsed = started.elapsed();
    let mean_us = elapsed.as_micros() / u128::from(N);
    std::fs::create_dir_all("/opt/cursor/artifacts").ok();
    std::fs::write(
        "/opt/cursor/artifacts/ipc_baseline.txt",
        format!("ping_roundtrip_mean_us={mean_us} n={N}\n"),
    )
    .unwrap();
    assert!(mean_us < 50_000, "unexpectedly slow ping mean_us={mean_us}");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}
