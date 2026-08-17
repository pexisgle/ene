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
        let bundled = PathBuf::from(&request.bundled_name);
        deny_remote(&bundled)?;
        let path = bundled_dir.join(&request.bundled_name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(BrokerError::SidecarBinaryNotFound)
}

pub(crate) fn spawn_child(
    uid: FiberUid,
    binary: &Path,
    args: &[String],
) -> Result<(LiveSidecar, SidecarId), BrokerError> {
    let port = allocate_loopback_port()?;
    let args = with_port(args, port);
    let mut child = Command::new(binary)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if !wait_healthy(port, &mut child) {
        terminate(&mut child);
        return Err(BrokerError::SidecarUnhealthy);
    }
    Ok((LiveSidecar { uid, child, port }, SidecarId::new()))
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

fn deny_remote(path: &Path) -> Result<(), BrokerError> {
    let raw = path.to_string_lossy();
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err(BrokerError::RemoteBinaryForbidden);
    }
    Ok(())
}

fn allocate_loopback_port() -> Result<u16, BrokerError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
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
