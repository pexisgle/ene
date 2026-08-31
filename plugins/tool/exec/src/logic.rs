use ene_plugin_ipc::ToolSpecWire;
use ene_registry::spec;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;

const STDOUT_CAP: usize = 1_048_576;
const STDERR_CAP: usize = 1_048_576;
const COMBINED_CAP: usize = 2_097_152;
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(2);
const KILL_WAIT: Duration = Duration::from_secs(1);

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "exec.run",
            "Run a process by program name (not a shell)",
            json!({"type":"object","properties":{"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"timeout_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}),
            vec!["exec".to_owned()],
        ),
        spec(
            "exec.shell",
            "Run a shell command string (higher risk than exec.run)",
            json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"},"timeout_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}),
            vec!["exec".to_owned(), "shell".to_owned()],
        ),
    ]
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "exec.run" => block_on_exec(run_direct(args)),
        "exec.shell" => block_on_exec(run_shell(args)),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn block_on_exec<F>(fut: F) -> Result<Value, String>
where
    F: std::future::Future<Output = Result<Value, String>> + Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| err.to_string())?
                    .block_on(fut)
            })
            .join()
            .map_err(|_| "exec runtime thread panicked".to_owned())?
    })
}

async fn run_direct(args: &Value) -> Result<Value, String> {
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
    let mut cmd = Command::new(command);
    cmd.args(&extra);
    run_process(cmd, args).await
}

async fn run_shell(args: &Value) -> Result<Value, String> {
    let script = args
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing command".to_owned())?;
    if script.is_empty() {
        return Err("missing command".to_owned());
    }
    run_process(shell_command(script), args).await
}

fn shell_command(script: &str) -> Command {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }
    #[cfg(not(unix))]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", script]);
        cmd
    }
}

async fn run_process(mut cmd: Command, args: &Value) -> Result<Value, String> {
    let cwd = resolve_cwd(args)?;
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    apply_env(&mut cmd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000);
    let timeout = Duration::from_millis(timeout_ms.max(1));
    let mut wrap = CommandWrap::from(cmd);
    #[cfg(unix)]
    wrap.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    wrap.wrap(JobObject);
    wrap.wrap(KillOnDrop);
    let mut child = wrap.spawn().map_err(|err| err.to_string())?;
    let captured = read_with_timeout(child.as_mut(), timeout).await?;
    Ok(output_value(&captured))
}

fn resolve_cwd(args: &Value) -> Result<Option<PathBuf>, String> {
    let workspace = workspace_root();
    let Some(workspace) = workspace else {
        return Ok(args.get("cwd").and_then(Value::as_str).map(PathBuf::from));
    };
    let raw = args.get("cwd").and_then(Value::as_str).unwrap_or(".");
    ene_registry::confine_tool_path(&workspace, Path::new(raw), false)
        .map(Some)
        .map_err(|err| err.to_string())
}

fn workspace_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_WORKSPACE.with(|slot| slot.borrow().clone()) {
        return Some(root);
    }
    std::env::var_os("ENE_WORKSPACE").map(PathBuf::from)
}

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static TEST_WORKSPACE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn apply_env(cmd: &mut Command) {
    cmd.env_clear();
    for (key, value) in std::env::vars_os() {
        let Some(key_str) = key.to_str() else {
            continue;
        };
        if env_is_allowed(key_str) && !env_is_secret(key_str) {
            cmd.env(key, value);
        }
    }
}

fn env_is_allowed(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "USER"
            | "LANG"
            | "TERM"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "SystemRoot"
            | "WINDIR"
            | "USERPROFILE"
            | "PATHEXT"
            | "COMSPEC"
    ) || key.starts_with("LC_")
}

fn env_is_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("API_KEY")
        || upper.starts_with("AWS_")
        || upper.ends_with("_KEY")
}

struct CapturedOutput {
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
    truncated_bytes: u64,
}

struct CapBuffers {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
    truncated_bytes: u64,
    combined_used: usize,
}

impl CapBuffers {
    fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            truncated_bytes: 0,
            combined_used: 0,
        }
    }

    fn push_stdout(&mut self, chunk: &[u8]) {
        self.push_stream(chunk, true);
    }

    fn push_stderr(&mut self, chunk: &[u8]) {
        self.push_stream(chunk, false);
    }

    fn push_stream(&mut self, chunk: &[u8], stdout: bool) {
        if chunk.is_empty() {
            return;
        }
        let stream_cap = if stdout { STDOUT_CAP } else { STDERR_CAP };
        let (buf, truncated_flag) = if stdout {
            (&mut self.stdout, &mut self.stdout_truncated)
        } else {
            (&mut self.stderr, &mut self.stderr_truncated)
        };
        let mut offset = 0;
        while offset < chunk.len() {
            if self.combined_used >= COMBINED_CAP {
                self.truncated_bytes += u64::try_from(chunk.len() - offset).unwrap_or(0);
                *truncated_flag = true;
                return;
            }
            let stream_room = stream_cap.saturating_sub(buf.len());
            let combined_room = COMBINED_CAP - self.combined_used;
            let room = stream_room.min(combined_room);
            if room == 0 {
                self.truncated_bytes += u64::try_from(chunk.len() - offset).unwrap_or(0);
                *truncated_flag = true;
                return;
            }
            let take = room.min(chunk.len() - offset);
            buf.extend_from_slice(&chunk[offset..offset + take]);
            self.combined_used += take;
            offset += take;
        }
    }
}

async fn read_with_timeout(
    child: &mut dyn ChildWrapper,
    timeout: Duration,
) -> Result<CapturedOutput, String> {
    let stdout = child
        .stdout()
        .take()
        .ok_or_else(|| "stdout pipe missing".to_owned())?;
    let stderr = child
        .stderr()
        .take()
        .ok_or_else(|| "stderr pipe missing".to_owned())?;
    let caps = Arc::new(Mutex::new(CapBuffers::new()));
    let stdout_caps = Arc::clone(&caps);
    let stdout_task = tokio::spawn(async move { read_stream(stdout, stdout_caps, true).await });
    let stderr_caps = Arc::clone(&caps);
    let stderr_task = tokio::spawn(async move { read_stream(stderr, stderr_caps, false).await });
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            drop(stdout_task.await);
            drop(stderr_task.await);
            snapshot(false, status.code(), &caps)
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => finish_timeout(child, &caps, stdout_task, stderr_task).await,
    }
}

async fn read_stream(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    caps: Arc<Mutex<CapBuffers>>,
    stdout: bool,
) {
    let mut buf = vec![0_u8; 8192];
    loop {
        let read = match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Ok(mut caps) = caps.lock() {
            if stdout {
                caps.push_stdout(&buf[..read]);
            } else {
                caps.push_stderr(&buf[..read]);
            }
        }
    }
}

async fn finish_timeout(
    child: &mut dyn ChildWrapper,
    caps: &Arc<Mutex<CapBuffers>>,
    stdout_task: tokio::task::JoinHandle<()>,
    stderr_task: tokio::task::JoinHandle<()>,
) -> Result<CapturedOutput, String> {
    #[cfg(unix)]
    {
        drop(child.signal(libc::SIGTERM));
        match tokio::time::timeout(TERM_GRACE, child.wait()).await {
            Ok(Ok(status)) => {
                drop(stdout_task.await);
                drop(stderr_task.await);
                return snapshot(true, status.code(), caps);
            }
            Ok(Err(err)) => return Err(err.to_string()),
            Err(_) => {}
        }
    }
    drop(child.start_kill());
    match tokio::time::timeout(KILL_WAIT, child.wait()).await {
        Ok(Ok(status)) => {
            drop(stdout_task.await);
            drop(stderr_task.await);
            snapshot(true, status.code(), caps)
        }
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err("exec timed out".to_owned()),
    }
}

fn snapshot(
    timed_out: bool,
    exit_code: Option<i32>,
    caps: &Arc<Mutex<CapBuffers>>,
) -> Result<CapturedOutput, String> {
    let caps = caps.lock().map_err(|_| "output lock poisoned".to_owned())?;
    Ok(CapturedOutput {
        timed_out,
        exit_code,
        stdout: caps.stdout.clone(),
        stderr: caps.stderr.clone(),
        stdout_truncated: caps.stdout_truncated,
        stderr_truncated: caps.stderr_truncated,
        truncated_bytes: caps.truncated_bytes,
    })
}

fn output_value(output: &CapturedOutput) -> Value {
    json!({
        "timed_out": output.timed_out,
        "exit_code": output.exit_code,
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
        "stdout_truncated": output.stdout_truncated,
        "stderr_truncated": output.stderr_truncated,
        "truncated_bytes": output.truncated_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::{TEST_WORKSPACE, execute};
    use serde_json::json;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::process::Command;
    use tempfile::TempDir;

    fn with_workspace<F: FnOnce()>(f: F) {
        let dir = TempDir::new().unwrap();
        TEST_WORKSPACE.with(|slot| {
            *slot.borrow_mut() = Some(dir.path().to_path_buf());
        });
        f();
        TEST_WORKSPACE.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }

    #[test]
    fn rejects_path_command() {
        let err = execute("exec.run", &json!({"command": "/bin/echo"})).unwrap_err();
        assert!(err.contains("program name"));
    }

    #[test]
    fn exec_run_and_shell_specs_differ() {
        let run = super::specs()
            .into_iter()
            .find(|spec| spec.name == "exec.run")
            .unwrap();
        let shell = super::specs()
            .into_iter()
            .find(|spec| spec.name == "exec.shell")
            .unwrap();
        assert_eq!(run.side_effects, vec!["exec".to_owned()]);
        assert_eq!(
            shell.side_effects,
            vec!["exec".to_owned(), "shell".to_owned()]
        );
    }

    #[test]
    fn stdout_over_cap_sets_truncation_metadata() {
        with_workspace(|| {
            let value = execute(
                "exec.run",
                &json!({
                    "command": "python3",
                    "args": ["-c", "import sys; sys.stdout.write('a' * 1048676)"],
                    "timeout_ms": 5000
                }),
            )
            .unwrap();
            assert_eq!(value["stdout_truncated"], json!(true));
            assert!(value["truncated_bytes"].as_u64().unwrap_or(0) > 0);
            assert!(value["stdout"].as_str().unwrap_or("").len() <= 1_048_576);
        });
    }

    #[test]
    fn cwd_outside_workspace_is_denied() {
        with_workspace(|| {
            let err = execute("exec.run", &json!({"command": "pwd", "cwd": "/tmp"})).unwrap_err();
            assert!(err.contains("PathEscape") || err.contains("escape"));
        });
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cwd_escape_is_denied() {
        with_workspace(|| {
            let root = TEST_WORKSPACE.with(|slot| slot.borrow().clone()).unwrap();
            let outside = TempDir::new().unwrap();
            let link = root.join("escape-link");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            let err =
                execute("exec.run", &json!({"command": "pwd", "cwd": "escape-link"})).unwrap_err();
            assert!(err.contains("PathEscape") || err.contains("escape"));
        });
    }

    #[test]
    fn secret_env_is_not_inherited() {
        assert!(super::env_is_secret("OPENAI_API_KEY"));
        assert!(super::env_is_secret("AWS_SECRET_ACCESS_KEY"));
        assert!(!super::env_is_secret("PATH"));
        assert!(super::env_is_allowed("PATH"));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_child_and_grandchild() {
        with_workspace(|| {
            let root = TEST_WORKSPACE.with(|slot| slot.borrow().clone()).unwrap();
            let marker = root.join("grandchild.pid");
            drop(fs::remove_file(&marker));
            let script = format!("sleep 120 & echo $! > {}; exec sleep 120", marker.display());
            let value = execute(
                "exec.run",
                &json!({
                    "command": "sh",
                    "args": ["-c", script],
                    "timeout_ms": 300
                }),
            )
            .unwrap();
            assert_eq!(value["timed_out"], json!(true));
            let pid_text = fs::read_to_string(&marker).unwrap_or_default();
            let pid = pid_text.trim().parse::<i32>().unwrap_or(0);
            assert!(pid > 0);
            assert!(!pid_alive(pid));
        });
    }

    #[cfg(unix)]
    fn pid_alive(pid: i32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(unix)]
    #[test]
    fn timeout_sends_term_and_returns_partial() {
        with_workspace(|| {
            let value = execute(
                "exec.run",
                &json!({
                    "command": "sleep",
                    "args": ["30"],
                    "timeout_ms": 200
                }),
            )
            .unwrap();
            assert_eq!(value["timed_out"], json!(true));
        });
    }
}
