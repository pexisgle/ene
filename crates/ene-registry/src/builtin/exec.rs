use super::spec;
use ene_plugin_ipc::ToolSpecWire;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(super) fn specs() -> Vec<ToolSpecWire> {
    vec![spec(
        "exec.run",
        "Run a process in the workspace (not a shell)",
        json!({"type":"object","properties":{"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}),
        vec!["exec".to_owned()],
    )]
}

pub(super) fn execute(args: &Value) -> Result<Value, String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing command".to_owned())?;
    if command.is_empty() || command.contains('/') || command.contains('\\') {
        return Err("command must be a program name, not a path".to_owned());
    }
    let extra: Vec<String> = args
        .get("args")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ENE_WORKSPACE").map(PathBuf::from));
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);
    let mut cmd = Command::new(command);
    cmd.args(&extra);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|err| err.to_string())?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(tx.send(child.wait_with_output()));
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms.max(1))) {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Ok(json!({
                "exit_code": output.status.code().unwrap_or(1),
                "stdout": stdout,
                "stderr": stderr,
            }))
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => {
            drop(Command::new("kill").args(["-9", &pid.to_string()]).status());
            Err("exec timed out".to_owned())
        }
    }
}
