use crate::SidecarRequest;
use ene_provider_assets::resolve_active_path;
use serde_json::{Value, json};

use crate::broker::{Broker, BrokerError};
use crate::fiber::FiberUid;
use crate::sidecar::SidecarId;
use crate::supervisor::SupervisorError;

const GGUF_PLUGIN: &str = "provider.gguf";
const LLAMA_ASSET: &str = "llama-server";

#[must_use]
pub fn sidecar_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Inject `sidecar_base_url` when a managed `llama-server` binary is installed.
///
/// # Errors
///
/// Returns when the broker cannot spawn or health-check the sidecar.
pub fn inject_gguf_sidecar(
    plugin: &str,
    config: &Value,
    uid: FiberUid,
    broker: &mut Broker,
) -> Result<Value, SupervisorError> {
    if plugin != GGUF_PLUGIN {
        return Ok(config.clone());
    }
    let mut out = config.clone();
    if out
        .get("sidecar_base_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty())
    {
        return Ok(out);
    }
    let binary = resolve_sidecar_binary(config)?;
    let Some(binary) = binary else {
        return Ok(out);
    };
    broker.grant(uid, "proc.spawn_sidecar");
    let model_path = out
        .get("model_path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut args = vec![
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        "{port}".to_owned(),
    ];
    if !model_path.is_empty() {
        args.push("-m".to_owned());
        args.push(model_path);
    }
    if let Some(custom) = out.get("server_args").and_then(Value::as_array) {
        if !custom.is_empty() {
            args = custom
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
    }
    let request = SidecarRequest {
        config_path: Some(binary),
        cas_path: None,
        bundled_name: String::new(),
        args,
    };
    let id = broker
        .spawn_sidecar(uid, &request)
        .map_err(map_broker_err)?;
    let health = broker.sidecar_health(uid, id).map_err(map_broker_err)?;
    if !health.alive {
        let _ignored = broker.kill_sidecar(uid, id);
        return Err(SupervisorError::Spawn(
            "llama-server sidecar did not become healthy".to_owned(),
        ));
    }
    out["sidecar_base_url"] = json!(format!("http://127.0.0.1:{}/v1", health.port));
    out["sidecar_id"] = json!(id.to_string());
    Ok(out)
}

fn resolve_sidecar_binary(config: &Value) -> Result<Option<std::path::PathBuf>, SupervisorError> {
    for key in ["server_path", "cas_path"] {
        if let Some(raw) = config.get(key).and_then(Value::as_str) {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let path = std::path::PathBuf::from(trimmed);
            if path.is_file() {
                return Ok(Some(path));
            }
            return Err(SupervisorError::Spawn(format!(
                "configured sidecar binary missing: {trimmed}"
            )));
        }
    }
    Ok(resolve_active_path(
        GGUF_PLUGIN,
        LLAMA_ASSET,
        sidecar_binary_name(),
    ))
}

fn map_broker_err(err: BrokerError) -> SupervisorError {
    SupervisorError::Spawn(err.to_string())
}

#[allow(dead_code)]
pub type GgufSidecarId = SidecarId;
