use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ene_plugin_ipc::{HostConn, HostHello, ProtoId, ProtocolRanges};
use ene_sandbox::SandboxSpec;
use tokio::net::UnixListener;
use tokio::time::timeout;
use uuid::Uuid;

use crate::supervisor::SupervisorError;

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SpawnedPlugin {
    child: Option<Child>,
    conn: Option<HostConn<tokio::net::UnixStream>>,
}

impl SpawnedPlugin {
    pub(crate) fn take(
        &mut self,
    ) -> Result<(Child, HostConn<tokio::net::UnixStream>), SupervisorError> {
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
    pub socket_dir: &'a Path,
    pub row_id: &'a str,
    pub sandbox_required: bool,
    pub temp_dir: &'a Path,
    pub workspace: &'a Path,
    pub config: &'a serde_json::Value,
    pub max_frame_bytes: u32,
    pub allow_unverified: bool,
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
    std::fs::create_dir_all(opts.socket_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(opts.socket_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::create_dir_all(opts.temp_dir)?;
    let socket_path = opts.socket_dir.join(format!("{}.sock", opts.row_id));
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    let spawn_token = Uuid::now_v7().to_string();
    let sandbox = sandbox_for(&opts)?;
    let mut command = plugin_command(
        opts.binary,
        &socket_path,
        opts.temp_dir,
        &spawn_token,
        opts.workspace,
        opts.config,
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
            terminate_child(&mut child);
            return Err(SupervisorError::Spawn(
                "hello timeout waiting for plugin connect".to_owned(),
            ));
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

fn plugin_command(
    binary: &Path,
    socket_path: &Path,
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
    cmd.env("ENE_PLUGIN_SOCKET", socket_path);
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

fn sandbox_for(opts: &SpawnOpts<'_>) -> Result<Option<SandboxSpec>, SupervisorError> {
    if !opts.sandbox_required {
        return Ok(None);
    }
    if !ene_sandbox::supported() {
        return Err(SupervisorError::SandboxRequired);
    }
    Ok(Some(build_spec(
        opts.binary,
        opts.socket_dir,
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
    socket_dir: &Path,
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
    allowed_read.push(socket_dir.to_path_buf());
    allowed_read.push(temp_dir.to_path_buf());
    allowed_write.push(socket_dir.to_path_buf());
    allowed_write.push(temp_dir.to_path_buf());
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    allowed_read.push(workspace.clone());
    allowed_write.push(workspace);
    if let Some(parent) = binary.parent() {
        allowed_read.push(parent.to_path_buf());
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
    vec![dir.join(stem), dir.join("plugins").join(stem)]
}

#[cfg(test)]
mod tests {
    use super::plugin_isolates_network;

    #[test]
    fn web_plugin_keeps_host_network() {
        assert!(!plugin_isolates_network("tool.web"));
        assert!(plugin_isolates_network("tool.fs"));
        assert!(plugin_isolates_network("tool.exec"));
    }
}
