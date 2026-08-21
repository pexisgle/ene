use crate::{
    HostConn, HostHello, IpcError, PluginConfigApplyResult, PluginConfigError, PluginConfigOption,
    PluginConfigOptionsResult, PluginConfigSchema, PluginConfigValidateResult, ProtoId,
    ProtocolRanges, ToolHandler, ToolSpecWire, redact_config_values, serve_plugin,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::io::DuplexStream;

const SPAWN_TOKEN: &str = "test-spawn-token";

struct BareHandler;

#[async_trait]
#[expect(
    clippy::unnecessary_literal_bound,
    reason = "test stand-in returns fixed plugin identity strings"
)]
impl ToolHandler for BareHandler {
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
            description: "hash".to_owned(),
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
        _name: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError> {
        Ok(json!({}))
    }
    fn spawn_token(&self) -> Result<String, String> {
        Ok(SPAWN_TOKEN.to_owned())
    }
}

struct SecretHandler;

#[async_trait]
#[expect(
    clippy::unnecessary_literal_bound,
    reason = "test stand-in returns fixed plugin identity strings"
)]
impl ToolHandler for SecretHandler {
    fn plugin_id(&self) -> &str {
        "tool.demo"
    }
    fn plugin_name(&self) -> &str {
        "demo"
    }
    fn digest(&self) -> &str {
        "sha256:test"
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        Vec::new()
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
    fn has_config(&self) -> bool {
        true
    }
    async fn config_schema(&self) -> Result<PluginConfigSchema, IpcError> {
        Ok(in_process_schema())
    }
    async fn config_validate(
        &self,
        values: serde_json::Value,
    ) -> Result<PluginConfigValidateResult, IpcError> {
        if values.get("model").and_then(serde_json::Value::as_str) == Some("bad") {
            return Ok(PluginConfigValidateResult {
                ok: false,
                errors: vec![PluginConfigError {
                    path: "model".to_owned(),
                    message: "unknown model".to_owned(),
                }],
                restart_required: false,
            });
        }
        Ok(PluginConfigValidateResult::ok())
    }
    async fn config_options(&self, field: &str) -> Result<PluginConfigOptionsResult, IpcError> {
        if field == "model" {
            return Ok(PluginConfigOptionsResult {
                options: vec![PluginConfigOption {
                    id: "ok".to_owned(),
                    label: "ok".to_owned(),
                }],
                error: None,
                fallback: false,
            });
        }
        Ok(PluginConfigOptionsResult::unsupported())
    }
    async fn config_apply(
        &self,
        values: serde_json::Value,
    ) -> Result<PluginConfigApplyResult, IpcError> {
        if values.get("model").and_then(serde_json::Value::as_str) == Some("bad") {
            return Ok(PluginConfigApplyResult {
                ok: false,
                errors: vec![PluginConfigError {
                    path: "model".to_owned(),
                    message: "unknown model".to_owned(),
                }],
                restart_required: false,
            });
        }
        Ok(PluginConfigApplyResult::ok(true))
    }
}

fn in_process_schema() -> PluginConfigSchema {
    PluginConfigSchema {
        has_config: true,
        schema: json!({
            "type": "object",
            "properties": {
                "model": { "type": "string" },
                "api_key": { "type": "string", "x-ene-secret": true }
            }
        }),
        secret_keys: vec!["api_key".to_owned()],
    }
}

fn hello() -> crate::HostHello {
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

async fn connect(handler: impl ToolHandler + 'static) -> HostConn<DuplexStream> {
    let (host_side, plugin_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        drop(serve_plugin(plugin_side, handler).await);
    });
    HostConn::handshake(
        host_side,
        hello(),
        &[ProtoId::Core, ProtoId::Tool],
        SPAWN_TOKEN,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn plugin_without_config_needs_no_extra_impl() {
    let mut host = connect(BareHandler).await;
    assert!(!host.has_config());
    let schema = host.config_schema().await.unwrap();
    assert!(!schema.has_config);
    let validated = host
        .config_validate(json!({"ignored": true}))
        .await
        .unwrap();
    assert!(validated.ok);
    let applied = host.config_apply(json!({"ignored": true})).await.unwrap();
    assert!(applied.ok);
}

#[tokio::test]
async fn schema_and_validation_match_in_process_and_ipc() {
    let direct = SecretHandler.config_schema().await.unwrap();
    let mut host = connect(SecretHandler).await;
    assert!(host.has_config());
    let via_ipc = host.config_schema().await.unwrap();
    assert_eq!(direct, via_ipc);
    let bad = host.config_validate(json!({"model": "bad"})).await.unwrap();
    assert!(!bad.ok);
    assert_eq!(bad.errors[0].path, "model");
    let options = host.config_options("model").await.unwrap();
    assert_eq!(options.options[0].id, "ok");
    let missing = host.config_options("missing").await.unwrap();
    assert!(missing.fallback);
}

#[tokio::test]
async fn secrets_are_stripped_from_schema_payloads_and_debug() {
    let schema = in_process_schema();
    let values = json!({ "model": "ok", "api_key": "sk-live-secret" });
    let redacted = redact_config_values(&schema.schema, &values);
    assert_eq!(redacted["model"], "ok");
    assert!(redacted.get("api_key").is_none());
    assert!(!format!("{redacted:?}").contains("sk-live"));
    assert!(!format!("{schema:?}").contains("sk-live"));
    let mut host = connect(SecretHandler).await;
    let via_ipc = host.config_schema().await.unwrap();
    assert!(!format!("{via_ipc:?}").contains("sk-live"));
}

#[tokio::test]
async fn apply_failure_does_not_report_success() {
    let mut host = connect(SecretHandler).await;
    let failed = host
        .config_apply(json!({"model": "bad", "api_key": "sk-live-secret"}))
        .await
        .unwrap();
    assert!(!failed.ok);
}
