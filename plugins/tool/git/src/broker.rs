//! Host-mediated process session for the git plugin.
//!
//! Every `git` invocation runs through the `Process` broker: the host
//! resolves the working directory against the plugin's `fs_grants`,
//! approves the argv, applies timeouts and output caps, and enforces the
//! implicit-download ban. The plugin never spawns a process or opens a
//! repository directly.

use std::sync::Arc;

use ene_plugin_broker::{BrokerClient, BrokerRequest};
use ene_plugin_proto::{HostServiceId, SandboxConfigData, ToolError};
use parking_lot::RwLock;
use tokio::sync::Mutex;

/// Default per-command timeout (milliseconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Default stdout/stderr cap per command (8 MiB, the host caps at 10 MiB).
const DEFAULT_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// One completed process run.
#[derive(Debug)]
pub struct GitRun {
    /// Exit code (`None` when the host reported none).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl GitRun {
    /// Whether the command succeeded.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Lazily-connected `Process` broker session shared by every action.
pub struct GitBroker {
    socket: RwLock<Option<String>>,
    token: RwLock<Option<String>>,
    client: Mutex<Option<BrokerClient>>,
}

impl GitBroker {
    /// A broker with no connection configuration yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            socket: RwLock::new(None),
            token: RwLock::new(None),
            client: Mutex::new(None),
        }
    }

    /// Captures the broker socket and auth token from the host sandbox
    /// config (protocol v8).
    pub fn configure(&self, sandbox: &SandboxConfigData) {
        self.socket.write().clone_from(&sandbox.broker_socket);
        self.token.write().clone_from(&sandbox.db_auth_token);
    }

    /// Runs `git <args>` with `cwd` as the working directory.
    ///
    /// The host resolves `cwd` against the plugin's `fs_grants`; a repo
    /// discovered outside those grants is rejected by the plugin's own
    /// [`RepoScope`](crate::sandbox::RepoScope) checks before any output is
    /// returned.
    pub async fn run_git(&self, cwd: &str, arguments: &[&str]) -> Result<GitRun, ToolError> {
        let mut argv = Vec::with_capacity(arguments.len() + 1);
        argv.push("git".to_string());
        argv.extend(arguments.iter().map(|arg| (*arg).to_string()));
        let mut client = self
            .session()
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker connect failed: {e}")))?;
        let Some(client) = client.as_mut() else {
            return Err(ToolError::execution_failed(
                "broker client initialization failed",
            ));
        };
        let response = client
            .request(&BrokerRequest::ProcessSpawn {
                argv,
                cwd: Some(cwd.to_string()),
                env: Vec::new(),
                timeout_ms: DEFAULT_TIMEOUT_MS,
                max_output_bytes: DEFAULT_OUTPUT_BYTES,
            })
            .await
            .map_err(|e| ToolError::execution_failed(format!("broker request failed: {e}")))?;
        match response {
            ene_plugin_broker::BrokerResponse::ProcessSpawnOk {
                exit_code,
                stdout,
                stderr,
                ..
            } => Ok(GitRun {
                exit_code,
                stdout,
                stderr,
            }),
            other => Err(ToolError::execution_failed(format!(
                "unexpected broker response: {other:?}"
            ))),
        }
    }

    /// Opens the broker session on first use.
    async fn session(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<BrokerClient>>, ToolError> {
        let mut client = self.client.lock().await;
        if client.is_none() {
            let socket = self.socket.read().clone();
            let token = self.token.read().clone();
            let (Some(socket), Some(token)) = (socket, token) else {
                return Err(ToolError::execution_failed(
                    "broker channel is not configured (missing broker socket/token from the host)",
                ));
            };
            *client = Some(
                BrokerClient::connect(
                    std::path::Path::new(&socket),
                    &token,
                    HostServiceId::Process,
                )
                .await
                .map_err(|e| ToolError::execution_failed(format!("broker connect failed: {e}")))?,
            );
        }
        Ok(client)
    }
}

impl Default for GitBroker {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide broker handle; the host delivers socket/token via
/// `set_sandbox` before any request runs.
static BROKER_ARC: std::sync::OnceLock<Arc<GitBroker>> = std::sync::OnceLock::new();

/// Returns the shared broker, initializing the handle on first use.
pub(crate) fn broker() -> Arc<GitBroker> {
    Arc::clone(BROKER_ARC.get_or_init(|| Arc::new(GitBroker::new())))
}

/// Configures the shared broker from the host sandbox data.
pub(crate) fn configure_broker(sandbox: &SandboxConfigData) {
    broker().configure(sandbox);
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test fixture uses expect/panic for concise assertions"
)]
pub(crate) mod tests {
    use super::*;
    use ene_plugin_broker::{BrokerRequest, BrokerResponse, read_framed_json, write_framed_json};
    use ene_plugin_proto::{
        HostServiceRequest, HostServiceResponse, read_host_service_request,
        write_host_service_response,
    };

    /// Serializes tests that reconfigure the process-wide shared broker.
    pub static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Broker-frame mock of the host's `Process` passenger that executes
    /// real `git` invocations, so action tests exercise the full
    /// argv/cwd/stdout path against real repositories.
    pub struct MockGitBroker {
        socket: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    impl MockGitBroker {
        /// Spawns the mock on a fresh unix socket.
        #[must_use]
        pub fn spawn() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let socket = dir.path().join("git-mock.sock");
            let server = Self { socket, _dir: dir };
            tokio::spawn(run_server(server.socket.clone()));
            server
        }
    }

    /// Points the shared broker at `mock`'s socket, dropping any cached
    /// session first. Callers must hold [`TEST_SERIAL`].
    pub async fn configure_test_broker(mock: &MockGitBroker) {
        for _ in 0..200 {
            if mock.socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        broker().client.lock().await.take();
        broker().configure(&SandboxConfigData {
            broker_socket: Some(mock.socket.to_string_lossy().into_owned()),
            db_auth_token: Some("tok".to_string()),
            ..SandboxConfigData::default()
        });
    }

    async fn run_server(socket: std::path::PathBuf) {
        let listener = tokio::net::UnixListener::bind(&socket).expect("mock bind");
        let (mut stream, _) = listener.accept().await.expect("mock accept");
        let open: HostServiceRequest = read_host_service_request(&mut stream)
            .await
            .expect("mock open")
            .expect("open frame");
        assert!(matches!(
            open,
            HostServiceRequest::Open {
                service: HostServiceId::Process,
                ..
            }
        ));
        write_host_service_response(&mut stream, &HostServiceResponse::OpenAck)
            .await
            .expect("mock ack");
        loop {
            let Ok(Some(request)) = read_framed_json::<_, BrokerRequest>(&mut stream).await else {
                return;
            };
            let BrokerRequest::ProcessSpawn {
                argv,
                cwd,
                env,
                timeout_ms,
                max_output_bytes,
                ..
            } = request
            else {
                panic!("expected ProcessSpawn, got {request:?}");
            };
            let Some(program) = argv.first() else {
                write_framed_json(
                    &mut stream,
                    &BrokerResponse::error(
                        ene_plugin_proto::BrokerErrorCode::InvalidTarget,
                        "empty argv",
                    ),
                )
                .await
                .expect("mock response");
                continue;
            };
            let mut command = tokio::process::Command::new(program);
            command.args(&argv[1..]);
            command.current_dir(cwd.as_deref().unwrap_or("."));
            command.env_clear();
            if let Some(path) = std::env::var_os("PATH") {
                command.env("PATH", path);
            }
            for (key, value) in env {
                command.env(key, value);
            }
            let Ok(Ok(output)) = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms.max(1)),
                command.output(),
            )
            .await
            else {
                write_framed_json(
                    &mut stream,
                    &BrokerResponse::error(
                        ene_plugin_proto::BrokerErrorCode::Denied,
                        "process timed out",
                    ),
                )
                .await
                .expect("mock response");
                continue;
            };
            let stdout_cap = usize::try_from(max_output_bytes).unwrap_or(usize::MAX);
            let mut stdout: Vec<u8> = output.stdout;
            stdout.truncate(stdout_cap);
            let mut stderr: Vec<u8> = output.stderr;
            stderr.truncate(stdout_cap);
            write_framed_json(
                &mut stream,
                &BrokerResponse::ProcessSpawnOk {
                    pid: 0,
                    exit_code: output.status.code(),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                },
            )
            .await
            .expect("mock response");
        }
    }
}
