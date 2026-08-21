use crate::{
    AssetView, AssetsHandler, CORE_VERSION, HostConn, HostHello, InstallAssetRequest,
    InstallAssetResult, InstallStatusRequest, InstallStatusResult, IpcError, ListAssetsResult,
    ListModelsRequest, ListModelsResult, LlmGenerateRequest, LlmGeneration, LlmHandler, LlmMessage,
    LlmRole, ModelsHandler, PluginIdentity, ProtoId, ProtocolRanges, ProviderAuth,
    ProviderHandlers, SetActiveAssetRequest, SetActiveAssetResult, ToolCall, ToolHandler,
    ToolSpecWire, VersionRange, serve_plugin, serve_provider,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::io::DuplexStream;

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
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
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
        max_frame_bytes: 0,
        allow_unverified: false,
    }
}

fn spawn_echo_plugin(plugin_side: DuplexStream) -> tokio::task::JoinHandle<Result<(), IpcError>> {
    tokio::spawn(async move { serve_plugin(plugin_side, EchoHandler).await })
}

#[tokio::test]
async fn ping_and_tool_call_roundtrip() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
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
    let (host_side, plugin_side) = tokio::io::duplex(1024);
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
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let (host_side, plugin_side) = tokio::io::duplex(1024);
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
    let (host_side, plugin_side) = tokio::io::duplex(1024);
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

#[tokio::test]
async fn digest_mismatch_rejects_unless_allow_unverified() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let plugin = spawn_echo_plugin(plugin_side);
    let mut denied = hello();
    denied.expected_digest = "sha256:other".to_owned();
    let err = HostConn::handshake(
        host_side,
        denied,
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        IpcError::Rejected(_) | IpcError::DigestMismatch
    ));
    drop(plugin);

    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let plugin = spawn_echo_plugin(plugin_side);
    let mut allowed = hello();
    allowed.expected_digest = "sha256:other".to_owned();
    allowed.allow_unverified = true;
    let mut host = HostConn::handshake(
        host_side,
        allowed,
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    host.ping().await.unwrap();
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

fn record_baseline(name: &str, body: impl AsRef<[u8]>) {
    let dir = std::path::Path::new("/opt/cursor/artifacts");
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    std::fs::write(dir.join(name), body).ok();
}

#[tokio::test]
async fn ipc_baseline_ping_is_measurable() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
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

struct FakeLlm;

#[async_trait]
impl LlmHandler for FakeLlm {
    async fn generate(&self, request: LlmGenerateRequest) -> Result<LlmGeneration, IpcError> {
        let last = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == LlmRole::User)
            .map_or("hello", |message| message.text.as_str());
        Ok(LlmGeneration {
            text: format!("llm:{last}"),
            model_id: request.model,
            finish_reason: "stop".to_owned(),
            ..LlmGeneration::default()
        })
    }
}

#[tokio::test]
async fn llm_generate_roundtrip_without_tool_face() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let identity = PluginIdentity {
        plugin_id: "provider.test".to_owned(),
        plugin_name: "test".to_owned(),
        digest: "sha256:test".to_owned(),
        spawn_token: Some(SPAWN_TOKEN.to_owned()),
    };
    let handlers = ProviderHandlers {
        llm: Some(std::sync::Arc::new(FakeLlm)),
        ..ProviderHandlers::default()
    };
    let plugin = tokio::spawn(async move { serve_provider(plugin_side, identity, handlers).await });
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Provider],
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let mut host = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Provider],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert!(host.negotiated().tool.is_none());
    assert_eq!(
        host.negotiated()
            .provider
            .as_ref()
            .and_then(|faces| faces.llm),
        Some(1)
    );
    let result = host
        .generate_llm(LlmGenerateRequest {
            messages: vec![LlmMessage {
                role: LlmRole::User,
                text: "ping".to_owned(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            }],
            tools: Vec::new(),
            model: "test-model".to_owned(),
            max_tokens: None,
            base_url: String::new(),
            auth: ProviderAuth::default(),
        })
        .await
        .unwrap();
    assert_eq!(result.text, "llm:ping");
    assert_eq!(result.model_id, "test-model");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

struct FakeModels;

#[async_trait]
impl ModelsHandler for FakeModels {
    async fn list_models(&self, request: ListModelsRequest) -> Result<ListModelsResult, IpcError> {
        Ok(ListModelsResult {
            models: vec![format!("{}:listed", request.seam)],
            error: None,
        })
    }
}

#[tokio::test]
async fn list_models_roundtrip() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let identity = PluginIdentity {
        plugin_id: "provider.test".to_owned(),
        plugin_name: "test".to_owned(),
        digest: "sha256:test".to_owned(),
        spawn_token: Some(SPAWN_TOKEN.to_owned()),
    };
    let handlers = ProviderHandlers {
        llm: Some(std::sync::Arc::new(FakeLlm)),
        models: Some(std::sync::Arc::new(FakeModels)),
        ..ProviderHandlers::default()
    };
    let plugin = tokio::spawn(async move { serve_provider(plugin_side, identity, handlers).await });
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Provider],
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let mut host = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Provider],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert_eq!(
        host.negotiated()
            .provider
            .as_ref()
            .and_then(|faces| faces.models),
        Some(1)
    );
    let listed = host
        .list_models(ListModelsRequest {
            seam: "seam.llm".to_owned(),
            base_url: String::new(),
            auth: ProviderAuth::default(),
        })
        .await
        .unwrap();
    assert_eq!(listed.models, vec!["seam.llm:listed".to_owned()]);
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[tokio::test]
async fn list_models_without_face_keeps_connection() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let identity = PluginIdentity {
        plugin_id: "provider.test".to_owned(),
        plugin_name: "test".to_owned(),
        digest: "sha256:test".to_owned(),
        spawn_token: Some(SPAWN_TOKEN.to_owned()),
    };
    let handlers = ProviderHandlers {
        llm: Some(std::sync::Arc::new(FakeLlm)),
        ..ProviderHandlers::default()
    };
    let plugin = tokio::spawn(async move { serve_provider(plugin_side, identity, handlers).await });
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Provider],
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let mut host = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Provider],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert!(
        host.negotiated()
            .provider
            .as_ref()
            .and_then(|faces| faces.models)
            .is_none()
    );
    let listed = host
        .list_models(ListModelsRequest {
            seam: "seam.llm".to_owned(),
            base_url: String::new(),
            auth: ProviderAuth::default(),
        })
        .await
        .unwrap();
    assert!(listed.models.is_empty());
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

struct FakeAssets;

#[async_trait]
impl AssetsHandler for FakeAssets {
    async fn list_assets(&self) -> Result<ListAssetsResult, IpcError> {
        Ok(ListAssetsResult {
            assets: vec![AssetView {
                id: "llama-server".to_owned(),
                kind: "sidecar".to_owned(),
                label: "llama-server".to_owned(),
                description: String::new(),
                recommended: true,
                installed: false,
                active: false,
                active_version: None,
                local_path: None,
                versions: Vec::new(),
                seams: Vec::new(),
            }],
            error: None,
        })
    }

    async fn install_asset(
        &self,
        _request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, IpcError> {
        Ok(InstallAssetResult {
            job_id: "job-1".to_owned(),
            error: None,
        })
    }

    async fn install_status(
        &self,
        _request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, IpcError> {
        Ok(InstallStatusResult::default())
    }

    async fn set_active(
        &self,
        _request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, IpcError> {
        Ok(SetActiveAssetResult { error: None })
    }
}

#[tokio::test]
async fn list_assets_roundtrip() {
    let (host_side, plugin_side) = tokio::io::duplex(1024);
    let identity = PluginIdentity {
        plugin_id: "provider.test".to_owned(),
        plugin_name: "test".to_owned(),
        digest: "sha256:test".to_owned(),
        spawn_token: Some(SPAWN_TOKEN.to_owned()),
    };
    let handlers = ProviderHandlers {
        assets: Some(std::sync::Arc::new(FakeAssets)),
        ..ProviderHandlers::default()
    };
    let plugin = tokio::spawn(async move { serve_provider(plugin_side, identity, handlers).await });
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: "sha256:test".to_owned(),
        declared_protocols: vec![ProtoId::Core, ProtoId::Provider],
        max_frame_bytes: 0,
        allow_unverified: false,
    };
    let mut host = HostConn::handshake(
        host_side,
        hello,
        &[ProtoId::Core, ProtoId::Provider],
        SPAWN_TOKEN,
    )
    .await
    .unwrap();
    assert_eq!(
        host.negotiated()
            .provider
            .as_ref()
            .and_then(|faces| faces.assets),
        Some(1)
    );
    let listed = host.list_assets().await.unwrap();
    assert_eq!(listed.assets.len(), 1);
    assert_eq!(listed.assets[0].id, "llama-server");
    host.drain().await.unwrap();
    plugin.await.unwrap().unwrap();
}

#[test]
fn tool_spec_wire_discovery_defaults_roundtrip() {
    let minimal = r#"{"name":"x","description":"d","parameters":{},"output":{},"side_effects":[]}"#;
    let decoded: ToolSpecWire = serde_json::from_str(minimal).unwrap();
    assert!(decoded.category.is_empty());
    assert!(decoded.keywords.is_empty());
    assert!(decoded.examples.is_empty());
    let encoded = serde_json::to_string(&decoded).unwrap();
    let again: ToolSpecWire = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, again);

    let full = r#"{"name":"x","description":"d","parameters":{},"output":{},"side_effects":[],"category":"fs","keywords":["read"],"examples":["read README"]}"#;
    let rich: ToolSpecWire = serde_json::from_str(full).unwrap();
    assert_eq!(rich.category, "fs");
    assert_eq!(rich.keywords, vec!["read".to_owned()]);
    assert_eq!(rich.examples, vec!["read README".to_owned()]);
}
