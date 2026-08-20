use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ene_plugin_ipc::{HostConn, HostHello, ProtoId, ProtocolRanges};
use ene_sandbox::SandboxSpec;
#[cfg(windows)]
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::time::timeout;
use uuid::Uuid;

use crate::supervisor::SupervisorError;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
type PluginListener = TcpListener;
#[cfg(unix)]
type PluginListener = UnixListener;

#[cfg(windows)]
type PluginStream = tokio::net::TcpStream;
#[cfg(unix)]
type PluginStream = tokio::net::UnixStream;

pub(crate) struct SpawnedPlugin {
    child: Option<Child>,
    conn: Option<HostConn<PluginStream>>,
}

impl SpawnedPlugin {
    pub(crate) fn take(&mut self) -> Result<(Child, HostConn<PluginStream>), SupervisorError> {
        let child = self
            .child
            .take()
            .ok_or_else(|| SupervisorError::Spawn("plugin child already taken".to_owned()))?;
        let conn = self
            .conn
            .take()
            .ok_or_else(|| SupervisorError::Spawn("plugin connection already taken".to_owned()))?;
        Ok((child, conn))
    }
}

impl Drop for SpawnedPlugin {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
        }
    }
}

pub(crate) struct SpawnOpts<'a> {
    pub binary: &'a Path,
    pub plugin_id: &'a str,
    pub digest: &'a str,
    pub row_id: &'a str,
    pub sandbox_required: bool,
    pub temp_dir: &'a Path,
    pub workspace: &'a Path,
    pub config: &'a serde_json::Value,
    pub max_frame_bytes: u32,
    pub allow_unverified: bool,
}

/// Plugin IPC socket path. Always a short `/tmp/ene-<hash>.sock` so bind
/// cannot hit `SUN_LEN` (108). Workspace and `TMPDIR` paths are often longer
/// than that once `probe-<uuid>.sock` is appended, especially when
/// `assets_dir` is still `target/debug/../../assets`.
///
/// The hash is `pid:row_id`, so concurrent spawns in one process must use
/// distinct row ids or they race on the same path.
#[cfg(unix)]
pub(crate) fn plugin_ipc_socket_path(row_id: &str) -> PathBuf {
    let key = format!("{}:{row_id}", std::process::id());
    let hex = format!("{}", blake3::hash(key.as_bytes()).to_hex());
    let name = format!("ene-{}.sock", hex.get(..16).unwrap_or(hex.as_str()));
    PathBuf::from("/tmp").join(name)
}

/// BLAKE3 digest of a plugin binary or script file (`blake3:<hex>`).
///
/// # Errors
///
/// Returns [`SupervisorError::Io`] when the file cannot be read.
pub fn file_digest(path: &Path) -> Result<String, SupervisorError> {
    let bytes = std::fs::read(path)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

pub(crate) async fn spawn_plugin(opts: SpawnOpts<'_>) -> Result<SpawnedPlugin, SupervisorError> {
    let (listener, endpoint, socket_path) = bind_plugin_listener(opts.row_id).await?;
    std::fs::create_dir_all(opts.temp_dir)?;
    let spawn_token = Uuid::now_v7().to_string();
    let sandbox = sandbox_for(&opts, &socket_path)?;
    let mut command = plugin_command(
        opts.binary,
        &endpoint,
        opts.temp_dir,
        &spawn_token,
        opts.workspace,
        opts.config,
    );
    tracing::debug!(
        plugin = opts.plugin_id,
        binary = %opts.binary.display(),
        endpoint,
        "spawning plugin"
    );
    apply_sandbox(&mut command, sandbox.as_ref())?;
    let mut child = command
        .spawn()
        .map_err(|err| SupervisorError::Spawn(err.to_string()))?;
    let accepted = timeout(HELLO_TIMEOUT, listener.accept()).await;
    let (stream, _) = match accepted {
        Ok(Ok(pair)) => pair,
        Ok(Err(err)) => {
            terminate_child(&mut child);
            return Err(err.into());
        }
        Err(_) => {
            let state = match child.try_wait()? {
                Some(status) => format!("plugin exited with {status}"),
                None => "plugin process is still running".to_owned(),
            };
            terminate_child(&mut child);
            return Err(SupervisorError::Spawn(format!(
                "hello timeout waiting for plugin connect ({state})"
            )));
        }
    };
    if child.try_wait()?.is_some() {
        return Err(SupervisorError::Spawn(
            "plugin exited before hello completed".to_owned(),
        ));
    }
    let declared = declared_protocols(opts.plugin_id);
    let hello = HostHello {
        host_name: "ene-core".to_owned(),
        host_version: "0.1.0".to_owned(),
        protocols: ProtocolRanges::host_supported(),
        expected_digest: opts.digest.to_owned(),
        declared_protocols: declared.clone(),
        max_frame_bytes: opts.max_frame_bytes,
        allow_unverified: opts.allow_unverified,
    };
    let handshake = timeout(
        HELLO_TIMEOUT,
        HostConn::handshake(stream, hello, &declared, &spawn_token),
    )
    .await;
    let conn = match handshake {
        Ok(Ok(conn)) => conn,
        Ok(Err(err)) => {
            terminate_child(&mut child);
            return Err(err.into());
        }
        Err(_) => {
            terminate_child(&mut child);
            return Err(SupervisorError::Spawn("hello timeout".to_owned()));
        }
    };
    if child.try_wait()?.is_some() {
        return Err(SupervisorError::Spawn(
            "plugin exited during hello".to_owned(),
        ));
    }
    tracing::debug!(plugin = opts.plugin_id, "plugin hello completed");
    Ok(SpawnedPlugin {
        child: Some(child),
        conn: Some(conn),
    })
}

#[cfg_attr(
    unix,
    expect(
        clippy::unused_async,
        reason = "Windows awaits TcpListener::bind; Unix bind is sync"
    )
)]
async fn bind_plugin_listener(
    #[cfg_attr(
        windows,
        expect(unused_variables, reason = "Windows binds an ephemeral TCP port")
    )]
    row_id: &str,
) -> Result<(PluginListener, String, PathBuf), SupervisorError> {
    #[cfg(unix)]
    {
        let socket_path = plugin_ipc_socket_path(row_id);
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(err) = std::fs::remove_file(&socket_path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err.into());
        }
        let listener = UnixListener::bind(&socket_path)?;
        let endpoint = socket_path.to_string_lossy().into_owned();
        Ok((listener, endpoint, socket_path))
    }
    #[cfg(windows)]
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?.to_string();
        let socket_path = PathBuf::from(&endpoint);
        Ok((listener, endpoint, socket_path))
    }
}

fn plugin_command(
    binary: &Path,
    endpoint: &str,
    temp_dir: &Path,
    spawn_token: &str,
    workspace: &Path,
    config: &serde_json::Value,
) -> Command {
    let mut cmd = if let Some(interpreter) = script_interpreter(binary) {
        let mut command = Command::new(interpreter);
        command.arg(binary);
        command
    } else {
        Command::new(binary)
    };
    cmd.env_clear();
    for key in [
        "PATH",
        "SystemRoot",
        "SystemDrive",
        "WINDIR",
        "COMSPEC",
        "PROGRAMDATA",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "LOCALAPPDATA",
        "HOME",
        "LANG",
        "TZ",
        "LD_LIBRARY_PATH",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "ALL_PROXY",
        "https_proxy",
        "http_proxy",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "RUST_LOG",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    cmd.env("ENE_PLUGIN_SOCKET", endpoint);
    cmd.env("ENE_PLUGIN_SPAWN_TOKEN", spawn_token);
    cmd.env("ENE_WORKSPACE", workspace);
    cmd.env("TMPDIR", temp_dir);
    if !config.is_null()
        && let Ok(encoded) = serde_json::to_string(config)
    {
        cmd.env("ENE_PROVIDER_CONFIG", encoded);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::inherit());
    cmd
}

fn declared_protocols(plugin_id: &str) -> Vec<ProtoId> {
    if plugin_id.starts_with("provider.") {
        vec![ProtoId::Core, ProtoId::Provider]
    } else {
        vec![ProtoId::Core, ProtoId::Tool]
    }
}

fn script_interpreter(path: &Path) -> Option<String> {
    if path.extension().is_some_and(|ext| ext == "py") {
        return Some("python3".to_owned());
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0_u8; 2];
    if file.read_exact(&mut header).is_err() || header != *b"#!" {
        return None;
    }
    let mut shebang = String::new();
    file.take(256).read_to_string(&mut shebang).ok()?;
    let line = shebang.lines().next()?;
    let interpreter = line.trim_start_matches('#').trim_start_matches('!');
    let program = interpreter.split_whitespace().next()?;
    Some(program.to_owned())
}

fn terminate_child(child: &mut Child) {
    if child.kill().is_err() {
        tracing::debug!("plugin child already gone");
    }
    drop(child.wait());
}

fn sandbox_for(
    opts: &SpawnOpts<'_>,
    socket_path: &Path,
) -> Result<Option<SandboxSpec>, SupervisorError> {
    if !opts.sandbox_required {
        return Ok(None);
    }
    if !ene_sandbox::supported() {
        return Err(SupervisorError::SandboxRequired);
    }
    Ok(Some(build_spec(
        opts.binary,
        opts.plugin_id,
        socket_path,
        opts.temp_dir,
        opts.workspace,
        opts.sandbox_required && plugin_isolates_network(opts.plugin_id),
    )))
}

fn plugin_isolates_network(plugin_id: &str) -> bool {
    plugin_id != "tool.web"
}

fn build_spec(
    binary: &Path,
    plugin_id: &str,
    socket_path: &Path,
    temp_dir: &Path,
    workspace: &Path,
    sandbox_required: bool,
) -> SandboxSpec {
    let mut allowed_read = Vec::new();
    let mut allowed_write = Vec::new();
    #[cfg(target_os = "linux")]
    {
        allowed_read.extend(ene_sandbox::linux::default_read_paths(binary));
        allowed_write.extend(ene_sandbox::linux::default_write_paths());
        if let Some(path) = std::env::var_os("PATH") {
            allowed_read.extend(std::env::split_paths(&path));
        }
    }
    allowed_read.push(socket_path.to_path_buf());
    allowed_read.push(temp_dir.to_path_buf());
    allowed_write.push(socket_path.to_path_buf());
    allowed_write.push(temp_dir.to_path_buf());
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    allowed_read.push(workspace.clone());
    allowed_write.push(workspace);
    if let Some(parent) = binary.parent() {
        allowed_read.push(parent.to_path_buf());
    }
    if plugin_id.starts_with("provider.") {
        allowed_read.push(
            ene_config::data_dir()
                .join("plugins")
                .join(plugin_id)
                .join("assets"),
        );
    }
    SandboxSpec {
        allowed_read_paths: allowed_read,
        allowed_write_paths: allowed_write,
        limits: ene_sandbox::ResourceLimits::default(),
        landlock: cfg!(target_os = "linux"),
        seccomp: cfg!(target_os = "linux"),
        no_new_privs: cfg!(target_os = "linux"),
        network_namespace: sandbox_required,
        cgroup: None,
        job_object: false,
    }
}

#[cfg_attr(
    not(target_os = "linux"),
    expect(
        clippy::unnecessary_wraps,
        reason = "the shared sandbox setup returns platform-specific errors"
    )
)]
fn apply_sandbox(command: &mut Command, spec: Option<&SandboxSpec>) -> Result<(), SupervisorError> {
    let Some(spec) = spec else {
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    {
        ene_sandbox::linux::prepare_command(command, spec)
            .map_err(|err| SupervisorError::Spawn(err.to_string()))?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, spec);
    }
    Ok(())
}

/// Resolve a profile plugin id to an executable path (native binary or script).
#[must_use]
pub fn discover_plugin_executable(plugin: &str) -> Option<PathBuf> {
    discover_plugin_executable_in(plugin, None)
}

/// Same as [`discover_plugin_executable`], also searching `plugins.home_dir`.
#[must_use]
pub fn discover_plugin_executable_in(plugin: &str, home: Option<&Path>) -> Option<PathBuf> {
    match plugin {
        "tool.utility" => discover_plugin_bin_in("ene-harness-utility", home),
        "tool.fs" => discover_plugin_bin_in("ene-harness-fs", home),
        "tool.exec" => discover_plugin_bin_in("ene-harness-exec", home),
        "tool.web" => discover_plugin_bin_in("ene-harness-web", home),
        "tool.app" => discover_plugin_bin_in("ene-harness-app", home),
        "tool.dummy" => discover_plugin_script("plugin.py"),
        other if other.starts_with("mcp.") => discover_plugin_bin_in("ene-harness-mcp", home),
        other => crate::providers::provider_plugin(other)
            .and_then(|meta| discover_plugin_bin_in(meta.bin, home)),
    }
}

#[must_use]
pub fn discover_plugin_bin(stem: &str) -> Option<PathBuf> {
    discover_plugin_bin_in(stem, None)
}

fn discover_plugin_bin_in(stem: &str, home: Option<&Path>) -> Option<PathBuf> {
    plugin_candidates(stem, home)
        .into_iter()
        .find(|path| path.is_file())
}

#[must_use]
pub fn discover_plugin_script(name: &str) -> Option<PathBuf> {
    plugin_candidates(name, None)
        .into_iter()
        .find(|path| path.is_file())
}

fn plugin_candidates(stem: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = home.filter(|path| !path.as_os_str().is_empty()) {
        candidates.extend(exe_plugin_candidates(home, stem));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.extend(exe_plugin_candidates(dir, stem));
        if let Some(parent) = dir.parent() {
            candidates.extend(exe_plugin_candidates(parent, stem));
        }
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut root = PathBuf::from(manifest);
        while root.parent().is_some() {
            candidates.push(root.join("target/debug").join(stem));
            candidates.push(root.join("target/release").join(stem));
            candidates.push(root.join("plugins/tool/dummy-py").join(stem));
            if root.join("Cargo.toml").is_file()
                && std::fs::read_to_string(root.join("Cargo.toml"))
                    .is_ok_and(|text| text.contains("[workspace]"))
            {
                break;
            }
            if !root.pop() {
                break;
            }
        }
    }
    candidates
}

#[must_use]
pub(crate) fn exe_plugin_candidates(dir: &Path, stem: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let exe = format!("{stem}.exe");
        vec![
            dir.join(stem),
            dir.join(&exe),
            dir.join("plugins").join(stem),
            dir.join("plugins").join(exe),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![dir.join(stem), dir.join("plugins").join(stem)]
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::plugin_ipc_socket_path;
    use super::plugin_isolates_network;

    #[test]
    fn web_plugin_keeps_host_network() {
        assert!(!plugin_isolates_network("tool.web"));
        assert!(plugin_isolates_network("tool.fs"));
        assert!(plugin_isolates_network("tool.exec"));
    }

    #[cfg(unix)]
    #[test]
    fn plugin_ipc_socket_path_fits_sun_len() {
        let path = plugin_ipc_socket_path("probe-019c4e2a-1234-7890-abcd-ef0123456789");
        assert!(
            path.as_os_str().len() < 40,
            "unix socket path too long: {} ({} bytes)",
            path.display(),
            path.as_os_str().len()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unnormalized_assets_workspace_socket_exceeds_sun_len() {
        let path = std::path::Path::new("/home/pexisgle/dev/Ene/target/debug")
            .join("../../assets/workspace/sockets")
            .join("probe-019c4e2a-1234-7890-abcd-ef0123456789.sock");
        assert!(
            path.as_os_str().len() >= 108,
            "unnormalized debug assets socket must exceed SUN_LEN: {} ({} bytes)",
            path.display(),
            path.as_os_str().len()
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_ipc_socket_path_differs_by_row_id() {
        assert_ne!(
            plugin_ipc_socket_path("r-dummy-exec"),
            plugin_ipc_socket_path("r-dummy-handshake")
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_ipc_socket_binds() {
        let path = plugin_ipc_socket_path("sun-len-bind-test");
        drop(std::fs::remove_file(&path));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);
        drop(std::fs::remove_file(&path));
    }
}
