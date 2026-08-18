//! Managed llama-server (or any `/v1` HTTP engine) spawned by this plugin.

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

static BASE: OnceLock<String> = OnceLock::new();
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Holds the child so Drop kills it when the plugin process exits `serve()`.
pub struct SidecarGuard;

impl Drop for SidecarGuard {
    fn drop(&mut self) {
        kill_current();
    }
}

#[must_use]
pub fn managed_base() -> Option<&'static str> {
    BASE.get().map(String::as_str)
}

/// Spawn a loopback engine when `server_path` or `cas_path` is set.
///
/// # Errors
///
/// Returns when the binary is missing, remote, or never becomes healthy.
pub fn maybe_start() -> Result<Option<SidecarGuard>, String> {
    maybe_start_with("/v1")
}

pub(crate) fn maybe_start_with(url_suffix: &str) -> Result<Option<SidecarGuard>, String> {
    let cfg = SidecarCfg::from_env();
    if cfg.server_path.trim().is_empty() && cfg.cas_path.trim().is_empty() {
        return Ok(None);
    }
    let binary = resolve(&cfg)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    drop(listener);
    let args = with_port(&cfg.argv(port), port);
    let mut child = Command::new(&binary)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| err.to_string())?;
    let timeout = Duration::from_secs(u64::from(cfg.timeout_secs.max(1)));
    if !wait_healthy(port, &mut child, timeout) {
        terminate(&mut child);
        return Err("sidecar did not become healthy".to_owned());
    }
    let suffix = url_suffix.trim_end_matches('/');
    let base = format!("http://127.0.0.1:{port}{suffix}");
    drop(BASE.set(base));
    *lock_child() = Some(child);
    Ok(Some(SidecarGuard))
}

struct SidecarCfg {
    server_path: String,
    cas_path: String,
    model_path: String,
    server_args: Vec<String>,
    timeout_secs: u32,
}

impl SidecarCfg {
    fn from_env() -> Self {
        let value = std::env::var("ENE_PROVIDER_CONFIG")
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let timeout = value
            .get("startup_timeout_secs")
            .and_then(Value::as_u64)
            .and_then(|secs| u32::try_from(secs).ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(60);
        Self {
            server_path: string_field(&value, "server_path"),
            cas_path: string_field(&value, "cas_path"),
            model_path: string_field(&value, "model_path"),
            server_args: value
                .get("server_args")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            timeout_secs: timeout,
        }
    }

    fn argv(&self, port: u16) -> Vec<String> {
        if !self.server_args.is_empty() {
            return self.server_args.clone();
        }
        let mut args = vec![
            "--host".to_owned(),
            "127.0.0.1".to_owned(),
            "--port".to_owned(),
            port.to_string(),
        ];
        if !self.model_path.is_empty() {
            args.push("-m".to_owned());
            args.push(self.model_path.clone());
        }
        args
    }
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn resolve(cfg: &SidecarCfg) -> Result<PathBuf, String> {
    for raw in [&cfg.server_path, &cfg.cas_path] {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        deny_remote(trimmed)?;
        let path = PathBuf::from(trimmed);
        if path.is_file() {
            return Ok(path);
        }
        if !path.is_absolute()
            && let Some(found) = search_path(trimmed)
        {
            return Ok(found);
        }
        return Err(format!("sidecar binary missing: {trimmed}"));
    }
    Err("sidecar server_path is empty".to_owned())
}

fn deny_remote(raw: &str) -> Result<(), String> {
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file:") {
        return Err("sidecar download urls are not allowed".to_owned());
    }
    Ok(())
}

fn search_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

fn with_port(args: &[String], port: u16) -> Vec<String> {
    let text = port.to_string();
    args.iter()
        .map(|arg| arg.replace("{port}", &text))
        .collect()
}

fn wait_healthy(port: u16, child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return false;
        }
        if tcp_open(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tcp_open(port) && child.try_wait().ok().flatten().is_none()
}

fn tcp_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(80),
    )
    .is_ok()
}

fn lock_child() -> std::sync::MutexGuard<'static, Option<Child>> {
    CHILD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn kill_current() {
    if let Some(mut child) = lock_child().take() {
        terminate(&mut child);
    }
}

fn terminate(child: &mut Child) {
    drop(child.kill());
    drop(child.wait());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_http_binary_refs() {
        assert!(deny_remote("https://example.invalid/llama-server").is_err());
        assert!(deny_remote("/tmp/engine").is_ok());
    }

    #[test]
    fn substitutes_port_token() {
        let args = with_port(&["--port".into(), "{port}".into()], 1234);
        assert_eq!(args, vec!["--port", "1234"]);
    }

    #[test]
    fn default_argv_includes_model_path() {
        let cfg = SidecarCfg {
            server_path: "/bin/true".into(),
            cas_path: String::new(),
            model_path: "/models/x.gguf".into(),
            server_args: Vec::new(),
            timeout_secs: 5,
        };
        let argv = cfg.argv(9);
        assert!(argv.contains(&"-m".to_owned()));
        assert!(argv.contains(&"/models/x.gguf".to_owned()));
        assert!(argv.contains(&"9".to_owned()));
    }

    #[test]
    fn resolve_prefers_existing_config_path() {
        let dir = tempfile::TempDir::new().expect("temp");
        let binary = dir.path().join("engine");
        std::fs::write(&binary, b"x").expect("write");
        let cfg = SidecarCfg {
            server_path: binary.to_string_lossy().into_owned(),
            cas_path: String::new(),
            model_path: String::new(),
            server_args: Vec::new(),
            timeout_secs: 1,
        };
        assert_eq!(resolve(&cfg).expect("path"), binary);
    }
}
