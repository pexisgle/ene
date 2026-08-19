use crate::SidecarRequest;
use ene_provider_assets::resolve_active_binary;
use serde_json::{Value, json};

use crate::broker::{Broker, BrokerError};
use crate::fiber::FiberUid;
use crate::sidecar::SidecarId;
use crate::supervisor::SupervisorError;

const GGUF_PLUGIN: &str = "provider.gguf";
const VOICEVOX_PLUGIN: &str = "provider.voicevox";
const LLAMA_ASSET: &str = "llama-server";
const VOICEVOX_ASSET: &str = "voicevox-engine";

#[must_use]
#[expect(dead_code, reason = "reserved for manifest fallback naming")]
pub fn sidecar_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

#[must_use]
#[expect(dead_code, reason = "reserved for manifest fallback naming")]
pub fn voicevox_binary_name() -> &'static str {
    if cfg!(windows) {
        "run.exe"
    } else {
        "run"
    }
}

/// Inject managed sidecar config for host-catalog provider plugins.
///
/// # Errors
///
/// Returns when the broker cannot spawn or health-check a managed sidecar.
pub fn inject_managed_sidecar(
    plugin: &str,
    config: &Value,
    uid: FiberUid,
    broker: &mut Broker,
) -> Result<Value, SupervisorError> {
    match plugin {
        GGUF_PLUGIN => inject_gguf_sidecar(config, uid, broker),
        VOICEVOX_PLUGIN => Ok(inject_voicevox_sidecar(config)),
        _ => Ok(config.clone()),
    }
}

fn inject_gguf_sidecar(
    config: &Value,
    uid: FiberUid,
    broker: &mut Broker,
) -> Result<Value, SupervisorError> {
    let mut out = config.clone();
    if out
        .get("sidecar_base_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty())
    {
        return Ok(out);
    }
    let binary = match resolve_sidecar_override(config)? {
        Some(path) => Some(path),
        None => resolve_active_binary(GGUF_PLUGIN, LLAMA_ASSET),
    };
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
    if let Some(custom) = out.get("server_args").and_then(Value::as_array)
        && !custom.is_empty()
    {
        args = custom
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    let request = SidecarRequest {
        config_path: Some(binary),
        cas_path: None,
        bundled_name: String::new(),
        args,
    };
    let id = broker
        .spawn_sidecar(uid, &request)
        .map_err(|err| map_broker_err(&err))?;
    let health = broker.sidecar_health(uid, id).map_err(|err| map_broker_err(&err))?;
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

fn inject_voicevox_sidecar(config: &Value) -> Value {
    let mut out = config.clone();
    if out
        .get("server_path")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty())
    {
        return out;
    }
    if out
        .get("cas_path")
        .and_then(Value::as_str)
        .is_some_and(|path| !path.trim().is_empty())
    {
        return out;
    }
    if let Some(path) = resolve_active_binary(VOICEVOX_PLUGIN, VOICEVOX_ASSET) {
        out["cas_path"] = json!(path.display().to_string());
    }
    out
}

fn resolve_sidecar_override(config: &Value) -> Result<Option<std::path::PathBuf>, SupervisorError> {
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
    Ok(None)
}

fn map_broker_err(err: &BrokerError) -> SupervisorError {
    SupervisorError::Spawn(err.to_string())
}

#[expect(dead_code, reason = "legacy type alias for fiber sidecar ids")]
pub type GgufSidecarId = SidecarId;
