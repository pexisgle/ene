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
        Ok(Ok(output)) => Ok(output_value(&output, false)),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => finish_timeout(pid, &rx),
    }
}

fn finish_timeout(
    pid: u32,
    rx: &std::sync::mpsc::Receiver<std::io::Result<std::process::Output>>,
) -> Result<Value, String> {
    signal(pid, "TERM");
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(output)) => Ok(output_value(&output, true)),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => {
            signal(pid, "KILL");
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(output)) => Ok(output_value(&output, true)),
                Ok(Err(err)) => Err(err.to_string()),
                Err(_) => Err("exec timed out".to_owned()),
            }
        }
    }
}

fn output_value(output: &std::process::Output, timed_out: bool) -> Value {
    json!({
        "timed_out": timed_out,
        "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    })
}

fn signal(pid: u32, kind: &str) {
    let pid = pid.to_string();
    #[cfg(unix)]
    {
        let flag = if kind == "TERM" { "-TERM" } else { "-KILL" };
        drop(Command::new("kill").args([flag, &pid]).status());
    }
    #[cfg(not(unix))]
    {
        let mut args = vec!["/PID", pid.as_str(), "/T"];
        if kind != "TERM" {
            args.push("/F");
        }
        drop(Command::new("taskkill").args(args).status());
    }
}

#[cfg(test)]
mod tests {
    use super::execute;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn timeout_sends_term_and_returns_partial() {
        let value = execute(&json!({
            "command": "sleep",
            "args": ["30"],
            "timeout_ms": 200
        }))
        .unwrap();
        assert_eq!(value["timed_out"], json!(true));
    }

    #[test]
    fn rejects_path_command() {
        let err = execute(&json!({"command": "/bin/echo"})).unwrap_err();
        assert!(err.contains("program name"));
    }
}
