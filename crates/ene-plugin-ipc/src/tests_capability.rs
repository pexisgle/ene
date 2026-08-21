use crate::protocol::{CAPABILITY_VERSION, StreamOpen};
use crate::{
    CapabilityGrant, HostConn, HostHello, IpcError, ProtoId, ProtocolRanges, ToolHandler,
    ToolSpecWire, serve_plugin, should_spill,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::io::DuplexStream;

const SPAWN_TOKEN: &str = "test-spawn-token";

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
            parameters: json!({"type": "object"}),
            output: json!({"type": "object"}),
            side_effects: Vec::new(),
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
        }]
    }
    async fn call(
        &self,
        _name: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        Ok(json!({}))
    }
    fn spawn_token(&self) -> Result<String, String> {
        Ok(SPAWN_TOKEN.to_owned())
    }
}

fn capability_hello() -> HostHello {
    HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool, ProtoId::Capability],
        max_frame_bytes: 0,
        allow_unverified: false,
    }
}

fn spawn_plugin(plugin_side: DuplexStream) -> tokio::task::JoinHandle<Result<(), IpcError>> {
    tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await })
}

#[test]
fn host_advertises_capability() {
    let ranges = ProtocolRanges::host_supported();
    assert_eq!(
        ranges.capability.map(|range| range.max),
        Some(CAPABILITY_VERSION)
    );
}

#[test]
fn spill_threshold_uses_default_when_zero() {
    assert!(!should_spill(1_024, 0));
    assert!(should_spill(70_000, 0));
    assert!(should_spill(100, 50));
    assert!(!should_spill(50, 50));
}

#[test]
fn capability_messages_roundtrip() {
    let request = crate::protocol::Message::CapabilityRequest {
        id: 7,
        body: crate::protocol::CapabilityRequest {
            method: "fs.open_read".to_owned(),
            params: json!({"path": "a.txt"}),
            capability_ref: "cap-1".to_owned(),
        },
    };
    let bytes = request.encode().unwrap();
    let decoded = crate::protocol::Message::decode(&bytes).unwrap();
    assert_eq!(decoded.kind_name(), "capability_request");
    assert_eq!(decoded, request);

    let open = crate::protocol::Message::StreamOpen {
        id: 8,
        body: StreamOpen {
            stream_id: "s-8".to_owned(),
            kind: "audio".to_owned(),
        },
    };
    let decoded = crate::protocol::Message::decode(&open.encode().unwrap()).unwrap();
    assert_eq!(decoded.kind_name(), "stream_open");
}

#[tokio::test]
async fn capability_grant_and_stream_open_roundtrip() {
    let (host_side, plugin_side) = tokio::io::duplex(4096);
    let plugin = spawn_plugin(plugin_side);
    let mut host = HostConn::handshake(
        host_side,
        capability_hello(),
        &[ProtoId::Core, ProtoId::Tool, ProtoId::Capability],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert_eq!(host.negotiated().capability, Some(CAPABILITY_VERSION));
    let granted = host
        .grant_capability(CapabilityGrant {
            grant_id: "g-1".to_owned(),
            method: "fs.open_read".to_owned(),
            fd_count: 0,
            stream_id: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(granted.grant_id, "g-1");
    assert_eq!(granted.status, "applied");
    let opened = host.open_stream("audio").await.unwrap();
    assert!(opened.stream_id.starts_with("s-"));
    assert_eq!(opened.fd_count, 0);
    host.flow_control(&opened.stream_id, true).await.unwrap();
    host.release_capability("g-1").await.unwrap();
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn tool_plugin_without_capability_keeps_core_and_tool() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let plugin = spawn_plugin(plugin_side);
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Tool],
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let mut host = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert!(host.negotiated().capability.is_none());
    let err = host.open_stream("audio").await.unwrap_err();
    assert!(matches!(err, IpcError::Unexpected(_)));
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[cfg(unix)]
#[test]
fn scm_rights_passes_a_unix_socket() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    use crate::{recv_fds, send_fds};

    let (chan_a, chan_b) = UnixStream::pair().unwrap();
    let (mut payload_w, payload_r) = UnixStream::pair().unwrap();
    send_fds(&chan_a, &[payload_r.as_raw_fd()]).unwrap();
    drop(payload_r);
    let got = recv_fds(&chan_b).unwrap();
    assert_eq!(got.len(), 1);
    payload_w.write_all(b"bulk").unwrap();
    drop(payload_w);
    let mut received = UnixStream::from(got.into_iter().next().unwrap());
    let mut buf = [0_u8; 4];
    received.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"bulk");
}
