//! `llama-server` child process lifecycle.

use crate::config::{ProactiveAcceleration, ProactiveDecisionProviderConfig};
use crate::error::LlmProviderError;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

/// Launch parameters for a local decision server.
#[derive(Debug, Clone)]
pub struct LlamaServerSpec {
    /// Path to `llama-server`.
    pub executable: PathBuf,
    /// Path to GGUF weights.
    pub model_path: PathBuf,
    /// Acceleration preference.
    pub acceleration: ProactiveAcceleration,
    /// `"auto"` or integer GPU layers.
    pub gpu_layers: String,
    /// Context size.
    pub context_size: u32,
    /// Health wait timeout.
    pub startup_timeout: Duration,
}

impl LlamaServerSpec {
    /// Build a spec from proactive decision config after validating paths.
    pub fn from_config(cfg: &ProactiveDecisionProviderConfig) -> Result<Self, LlmProviderError> {
        let model_path = cfg.model_path.trim();
        if model_path.is_empty() {
            return Err(LlmProviderError::LocalProcess(
                "proactive decision model_path is empty".to_string(),
            ));
        }
        let model_path = PathBuf::from(model_path);
        if !model_path.is_file() {
            return Err(LlmProviderError::LocalProcess(format!(
                "decision model file not found: {}",
                model_path.display()
            )));
        }
        let executable = resolve_llama_server_executable(cfg.executable.trim())?;
        let startup_timeout = Duration::from_secs(cfg.startup_timeout_seconds.max(1));
        Ok(Self {
            executable,
            model_path,
            acceleration: cfg.acceleration,
            gpu_layers: cfg.gpu_layers.clone(),
            context_size: cfg.context_size.max(256),
            startup_timeout,
        })
    }
}

/// Resolve `llama-server` from an explicit path or `PATH`.
pub fn resolve_llama_server_executable(configured: &str) -> Result<PathBuf, LlmProviderError> {
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Ok(path);
        }
        return Err(LlmProviderError::LocalProcess(format!(
            "llama-server executable not found: {}",
            path.display()
        )));
    }
    which_binary("llama-server").ok_or_else(|| {
        LlmProviderError::LocalProcess(
            "llama-server not found on PATH; set provider.proactive.decision.executable"
                .to_string(),
        )
    })
}

fn which_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn free_loopback_port() -> Result<u16, LlmProviderError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| {
        LlmProviderError::LocalProcess(format!("failed to allocate loopback port: {e}"))
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| LlmProviderError::LocalProcess(format!("local_addr failed: {e}")))?
        .port();
    Ok(port)
}

/// Owned local server process bound to loopback.
pub struct LlamaServerHandle {
    child: Child,
    port: u16,
}

impl LlamaServerHandle {
    /// Spawn `llama-server` and wait until `/health` is ready (or timeout).
    pub async fn spawn(spec: LlamaServerSpec) -> Result<Self, LlmProviderError> {
        let port = free_loopback_port()?;
        let mut args = vec![
            "--model".to_string(),
            spec.model_path.display().to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            port.to_string(),
            "--ctx-size".to_string(),
            spec.context_size.to_string(),
            "--parallel".to_string(),
            "1".to_string(),
        ];
        append_acceleration_args(&mut args, spec.acceleration, &spec.gpu_layers);
        // Best-effort; older binaries ignore unknown flags after we may strip.
        args.push("--no-webui".to_string());

        tracing::info!(
            component = "LocalLlamaCpp",
            executable = %spec.executable.display(),
            port,
            acceleration = ?spec.acceleration,
            "Starting llama-server on loopback"
        );

        let mut command = Command::new(&spec.executable);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            LlmProviderError::LocalProcess(format!(
                "failed to spawn llama-server ({}): {e}",
                spec.executable.display()
            ))
        })?;

        pipe_child_logs(child.stdout.take(), child.stderr.take());

        let handle = Self { child, port };
        handle.wait_until_healthy(spec.startup_timeout).await?;
        Ok(handle)
    }

    /// Loopback TCP port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    async fn wait_until_healthy(&self, timeout: Duration) -> Result<(), LlmProviderError> {
        let url = format!("http://127.0.0.1:{}/health", self.port);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| LlmProviderError::LocalProcess(format!("http client: {e}")))?;
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            LlmProviderError::LocalProcess("startup timeout overflow".to_string())
        })?;
        loop {
            if Instant::now() >= deadline {
                return Err(LlmProviderError::LocalProcess(format!(
                    "llama-server health timeout after {timeout:?} ({url})"
                )));
            }
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Some builds return JSON with status=ok / ready.
                    if let Ok(body) = resp.text().await {
                        let lower = body.to_ascii_lowercase();
                        if lower.contains("error") && !lower.contains("\"ok\"") {
                            // Keep waiting unless clearly ready.
                            if lower.contains("loading") {
                                sleep(Duration::from_millis(250)).await;
                                continue;
                            }
                        }
                    }
                    return Ok(());
                }
                Ok(resp) => {
                    tracing::debug!(
                        component = "LocalLlamaCpp",
                        status = %resp.status(),
                        "llama-server health not ready"
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        component = "LocalLlamaCpp",
                        error = %e,
                        "llama-server health connect failed"
                    );
                }
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    /// Async shutdown: kill and wait.
    pub async fn shutdown(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    /// Best-effort kill from `Drop` paths (no async runtime required).
    pub fn kill_blocking(mut self) {
        let _ = self.child.start_kill();
    }
}

fn append_acceleration_args(
    args: &mut Vec<String>,
    acceleration: ProactiveAcceleration,
    gpu_layers: &str,
) {
    let layers = resolve_gpu_layers(gpu_layers);
    match acceleration {
        ProactiveAcceleration::Vulkan => {
            args.push("--device".to_string());
            args.push("Vulkan0".to_string());
            args.push("--n-gpu-layers".to_string());
            args.push(layers);
        }
        ProactiveAcceleration::Cuda => {
            args.push("--n-gpu-layers".to_string());
            args.push(layers);
        }
        ProactiveAcceleration::Cpu => {
            args.push("--n-gpu-layers".to_string());
            args.push("0".to_string());
        }
        ProactiveAcceleration::Auto => {
            // Prefer Vulkan on Linux when likely available; CUDA otherwise via layers.
            #[cfg(target_os = "linux")]
            {
                args.push("--device".to_string());
                args.push("Vulkan0".to_string());
            }
            args.push("--n-gpu-layers".to_string());
            args.push(layers);
        }
    }
}

fn resolve_gpu_layers(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        "99".to_string()
    } else {
        trimmed.to_string()
    }
}

fn pipe_child_logs(
    stdout: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    stderr: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
) {
    if let Some(out) = stdout {
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Never log prompt / screen payloads; server lines are status only.
                tracing::debug!(component = "llama-server", stream = "stdout", "{line}");
            }
        });
    }
    if let Some(err) = stderr {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(component = "llama-server", stream = "stderr", "{line}");
            }
        });
    }
}
