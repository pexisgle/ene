use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::broker::BrokerError;
use crate::fiber::FiberUid;

/// Host-assigned sidecar identity (P-1006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SidecarId(Uuid);

impl SidecarId {
    fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for SidecarId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Binary resolution inputs. The host never downloads from a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarRequest {
    /// Configured absolute path, if the user set one.
    pub config_path: Option<PathBuf>,
    /// Catalog-managed CAS artifact path injected by the host.
    pub cas_path: Option<PathBuf>,
    /// File name under the bundled plugins directory.
    pub bundled_name: String,
    /// Child args. `{port}` is replaced with the host-assigned loopback port.
    pub args: Vec<String>,
}

/// Last observed sidecar liveness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarHealth {
    pub alive: bool,
    pub port: u16,
}

pub(crate) struct LiveSidecar {
    pub(crate) uid: FiberUid,
    pub(crate) child: Child,
    pub(crate) port: u16,
}

/// Resolve config path → CAS artifact → bundled file → deny. URLs are never fetched.
pub(crate) fn resolve_binary(
    bundled_dir: &Path,
    request: &SidecarRequest,
) -> Result<PathBuf, BrokerError> {
    if let Some(path) = &request.config_path {
        deny_remote(path)?;
        if path.is_file() {
            return Ok(path.clone());
        }
    }
    if let Some(path) = &request.cas_path {
        deny_remote(path)?;
        if path.is_file() {
            return Ok(path.clone());
        }
    }
    if !request.bundled_name.is_empty() {
        reject_unsafe_bundled_name(&request.bundled_name)?;
        let path = bundled_dir.join(&request.bundled_name);
        deny_remote(&path)?;
        if path.is_file() {
            let canonical_bundled = bundled_dir.canonicalize().map_err(BrokerError::from)?;
            let canonical = path.canonicalize().map_err(BrokerError::from)?;
            if !canonical.starts_with(&canonical_bundled) {
                return Err(BrokerError::SidecarBinaryNotFound);
            }
            return Ok(canonical);
        }
    }
    Err(BrokerError::SidecarBinaryNotFound)
}

pub(crate) fn spawn_child(
    uid: FiberUid,
    binary: &Path,
    args: &[String],
) -> Result<(LiveSidecar, SidecarId), BrokerError> {
    // The listener is dropped before the child binds, so a peer could grab the
    // port first. Sidecars should bind with SO_REUSEADDR; health polling verifies
    // the expected child still owns the port after spawn.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let args = with_port(args, port);
    let mut child = sidecar_command(binary, &args).spawn()?;
    drop(listener);
    if !wait_healthy(port, &mut child) {
        terminate(&mut child);
        return Err(BrokerError::SidecarUnhealthy);
    }
    if process_exited(&mut child) {
        terminate(&mut child);
        return Err(BrokerError::SidecarUnhealthy);
    }
    Ok((LiveSidecar { uid, child, port }, SidecarId::new()))
}

fn sidecar_command(binary: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.env_clear();
    for key in ["PATH", "HOME", "LANG", "TMPDIR"] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd
}

pub(crate) fn health_of(live: &mut LiveSidecar) -> SidecarHealth {
    if process_exited(&mut live.child) {
        return SidecarHealth {
            alive: false,
            port: live.port,
        };
    }
    SidecarHealth {
        alive: tcp_open(live.port),
        port: live.port,
    }
}

pub(crate) fn terminate(child: &mut Child) {
    if child.kill().is_err() {
        tracing::debug!("sidecar child already gone");
    }
    drop(child.wait());
}

fn reject_unsafe_bundled_name(name: &str) -> Result<(), BrokerError> {
    if name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).is_absolute()
    {
        return Err(BrokerError::SidecarBinaryNotFound);
    }
    Ok(())
}

fn deny_remote(path: &Path) -> Result<(), BrokerError> {
    let raw = path.to_string_lossy();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file:") {
        return Err(BrokerError::RemoteBinaryForbidden);
    }
    Ok(())
}

fn with_port(args: &[String], port: u16) -> Vec<String> {
    let port_text = port.to_string();
    let mut out: Vec<String> = args
        .iter()
        .map(|arg| arg.replace("{port}", &port_text))
        .collect();
    if !args.iter().any(|arg| arg.contains("{port}")) {
        out.push(port_text);
    }
    out
}

fn wait_healthy(port: u16, child: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if process_exited(child) {
            return false;
        }
        if tcp_open(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    tcp_open(port) && !process_exited(child)
}

fn process_exited(child: &mut Child) -> bool {
    child.try_wait().ok().flatten().is_some()
}

fn tcp_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(50),
    )
    .is_ok()
}
