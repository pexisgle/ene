use crate::fiber::FiberUid;
use crate::sidecar::{self, LiveSidecar, SidecarHealth, SidecarId, SidecarRequest};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Broker denial or path escape.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BrokerError {
    #[error("denied {op} for fiber {uid}")]
    Denied { uid: String, op: String },
    #[error("path escapes workspace: {0}")]
    PathEscape(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sidecar binary not found")]
    SidecarBinaryNotFound,
    #[error("sidecar download urls are not allowed")]
    RemoteBinaryForbidden,
    #[error("sidecar {0} not found")]
    UnknownSidecar(String),
    #[error("sidecar did not become healthy")]
    SidecarUnhealthy,
    #[error("sidecar not owned by fiber {uid}")]
    SidecarNotOwned { uid: String },
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("ssrf: {0}")]
    Ssrf(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("response exceeds size limit")]
    Oversize,
    #[error("binary content is not allowed")]
    Binary,
    #[error("fetch timed out")]
    Timeout,
    #[error("too many redirects")]
    RedirectLoop,
    #[error("invalid glob: {0}")]
    InvalidGlob(String),
}

/// One grant tracked as a fiber effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub uid: FiberUid,
    pub op: String,
}

/// Resolve `path` under `workspace`. Parent directories are created only when
/// `create_parent` is true and the parent lies inside the canonical workspace.
///
/// # Errors
///
/// Returns [`BrokerError::PathEscape`] when the resolved path would leave the
/// workspace, and [`BrokerError::Io`] on filesystem failures.
pub fn confine_path(
    workspace: &Path,
    path: &Path,
    create_parent: bool,
) -> Result<PathBuf, BrokerError> {
    let base = workspace.canonicalize()?;
    if path.as_os_str().is_empty() || path == Path::new(".") {
        return Ok(base);
    }
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    if let Ok(canonical) = requested.canonicalize()
        && canonical == base
    {
        return Ok(base);
    }
    let file_name = requested
        .file_name()
        .ok_or_else(|| BrokerError::PathEscape(requested.display().to_string()))?;
    if file_name == Component::CurDir.as_os_str()
        || file_name == Component::ParentDir.as_os_str()
        || file_name.to_string_lossy().contains('/')
        || file_name.to_string_lossy().contains('\\')
    {
        return Err(BrokerError::PathEscape(requested.display().to_string()));
    }
    let parent = requested
        .parent()
        .ok_or_else(|| BrokerError::PathEscape(requested.display().to_string()))?;
    let canonical_parent = canonicalize_parent(&base, parent, create_parent)?;
    if !canonical_parent.starts_with(&base) {
        return Err(BrokerError::PathEscape(requested.display().to_string()));
    }
    let resolved = canonical_parent.join(file_name);
    if resolved.exists() {
        let canonical = resolved.canonicalize()?;
        if !canonical.starts_with(&base) {
            return Err(BrokerError::PathEscape(canonical.display().to_string()));
        }
        return Ok(canonical);
    }
    Ok(resolved)
}

fn canonicalize_parent(
    base: &Path,
    parent: &Path,
    create_parent: bool,
) -> Result<PathBuf, BrokerError> {
    if parent.exists() {
        return parent
            .canonicalize()
            .map_err(|_| BrokerError::PathEscape(parent.display().to_string()));
    }
    if !create_parent {
        return Err(BrokerError::PathEscape(parent.display().to_string()));
    }
    let mut existing = parent.to_path_buf();
    let mut missing: Vec<PathBuf> = Vec::new();
    while !existing.exists() {
        missing.push(existing.clone());
        let Some(next) = existing.parent() else {
            return Err(BrokerError::PathEscape(parent.display().to_string()));
        };
        existing = next.to_path_buf();
    }
    let anchor = existing.canonicalize()?;
    if !anchor.starts_with(base) {
        return Err(BrokerError::PathEscape(parent.display().to_string()));
    }
    missing.reverse();
    let mut current = anchor;
    for segment in missing {
        let Some(name) = segment.file_name() else {
            return Err(BrokerError::PathEscape(parent.display().to_string()));
        };
        if name == Component::ParentDir.as_os_str() || name == Component::CurDir.as_os_str() {
            return Err(BrokerError::PathEscape(parent.display().to_string()));
        }
        current = current.join(name);
        if !current.starts_with(base) {
            return Err(BrokerError::PathEscape(parent.display().to_string()));
        }
        if !current.exists() {
            std::fs::create_dir(&current)?;
        }
    }
    Ok(current)
}

/// Minimal fs/net/sidecar broker. Grants are per-fiber; undeclared ops are denied (I-48).
pub struct Broker {
    grants: HashMap<FiberUid, HashSet<String>>,
    workspace: PathBuf,
    bundled_dir: PathBuf,
    sidecars: HashMap<SidecarId, LiveSidecar>,
}

impl Broker {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self::with_bundled_dir(workspace, PathBuf::new())
    }

    /// `bundled_dir` is the last-resort sidecar binary location (never `PATH`).
    #[must_use]
    pub fn with_bundled_dir(workspace: PathBuf, bundled_dir: PathBuf) -> Self {
        Self {
            grants: HashMap::new(),
            workspace,
            bundled_dir,
            sidecars: HashMap::new(),
        }
    }

    pub fn grant(&mut self, uid: FiberUid, op: impl Into<String>) {
        self.grants.entry(uid).or_default().insert(op.into());
    }

    pub fn revoke_all(&mut self, uid: FiberUid) {
        self.grants.remove(&uid);
        let ids: Vec<SidecarId> = self
            .sidecars
            .iter()
            .filter(|(_, live)| live.uid == uid)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(mut live) = self.sidecars.remove(&id) {
                sidecar::terminate(&mut live.child);
            }
        }
    }

    #[must_use]
    pub fn has_grant(&self, uid: FiberUid, op: &str) -> bool {
        self.grants.get(&uid).is_some_and(|ops| ops.contains(op))
    }

    pub fn fs_read(&self, uid: FiberUid, path: &Path) -> Result<String, BrokerError> {
        self.require(uid, "fs.read")?;
        let resolved = confine_path(&self.workspace, path, false)?;
        std::fs::read_to_string(resolved).map_err(BrokerError::from)
    }

    pub fn fs_write(&self, uid: FiberUid, path: &Path, text: &str) -> Result<(), BrokerError> {
        self.require(uid, "fs.write")?;
        let resolved = confine_path(&self.workspace, path, true)?;
        std::fs::write(resolved, text).map_err(BrokerError::from)
    }

    pub fn net_fetch(&self, uid: FiberUid, url: &str) -> Result<Value, BrokerError> {
        self.require(uid, "net.fetch")?;
        crate::net::get(url)
    }

    /// Search file contents with host-managed `rg`.
    ///
    /// # Errors
    ///
    /// Denies undeclared `fs.search`, escapes the workspace, or propagates
    /// `rg` I/O failures. A literal query is passed as fixed strings; invalid
    /// regular expressions are rejected by `rg` before any result is returned.
    pub fn fs_search(
        &self,
        uid: FiberUid,
        path: &Path,
        query: &str,
        regex: bool,
        case_insensitive: bool,
        include: Option<&str>,
        context_lines: u32,
        count: bool,
        max: u32,
    ) -> Result<Value, BrokerError> {
        self.require(uid, "fs.search")?;
        let root = confine_path(&self.workspace, path, false)?;
        let include = match include {
            Some(pattern) => {
                validate_glob(pattern)?;
                Some(pattern)
            }
            None => None,
        };
        let mut command = std::process::Command::new("rg");
        if !regex {
            command.arg("--fixed-strings");
        }
        if case_insensitive {
            command.arg("--ignore-case");
        }
        if let Some(pattern) = include {
            command.args(["--glob", pattern]);
        }
        if context_lines > 0 {
            command.arg(format!("--context={context_lines}"));
        }
        if count {
            command.arg("--count");
            command.arg("--no-filename");
        }

        let max = usize::try_from(max).unwrap_or(200).min(200);
        command.arg(format!("--max-count={max}"));
        command
            .arg("--json")
            .arg("--")
            .arg(query)
            .arg(&root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = command.output()?;
        if !output.status.success() && output.status.code() != Some(1) {
            return Err(BrokerError::Fetch(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let matches = parse_rg_json(&stdout);
        serde_json::to_value(matches).map_err(|_| BrokerError::Fetch("serialize search".into()))
    }

    /// Resolve a sidecar binary without spawning it.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError::RemoteBinaryForbidden`] for `http(s)` refs and
    /// [`BrokerError::SidecarBinaryNotFound`] when no local file exists.
    pub fn resolve_sidecar_binary(&self, request: &SidecarRequest) -> Result<PathBuf, BrokerError> {
        sidecar::resolve_binary(&self.bundled_dir, request)
    }

    /// Spawn a loopback sidecar. The host assigns the port (P-1006).
    ///
    /// # Errors
    ///
    /// Denies undeclared `proc.spawn_sidecar`, missing binaries, and health
    /// probe timeouts. Remote URLs are never fetched.
    pub fn spawn_sidecar(
        &mut self,
        uid: FiberUid,
        request: &SidecarRequest,
    ) -> Result<SidecarId, BrokerError> {
        self.require(uid, "proc.spawn_sidecar")?;
        let binary = self.resolve_sidecar_binary(request)?;
        let (live, id) = sidecar::spawn_child(uid, &binary, &request.args)?;
        self.sidecars.insert(id, live);
        Ok(id)
    }

    /// Probe a sidecar the fiber owns.
    ///
    /// # Errors
    ///
    /// Unknown handles and cross-fiber access fail.
    pub fn sidecar_health(
        &mut self,
        uid: FiberUid,
        id: SidecarId,
    ) -> Result<SidecarHealth, BrokerError> {
        let live = self
            .sidecars
            .get_mut(&id)
            .ok_or_else(|| BrokerError::UnknownSidecar(id.to_string()))?;
        if live.uid != uid {
            return Err(BrokerError::SidecarNotOwned {
                uid: uid.to_string(),
            });
        }
        Ok(sidecar::health_of(live))
    }

    /// Kill a sidecar the fiber owns.
    ///
    /// # Errors
    ///
    /// Unknown handles and cross-fiber access fail.
    pub fn kill_sidecar(&mut self, uid: FiberUid, id: SidecarId) -> Result<(), BrokerError> {
        let live = self
            .sidecars
            .get(&id)
            .ok_or_else(|| BrokerError::UnknownSidecar(id.to_string()))?;
        if live.uid != uid {
            return Err(BrokerError::SidecarNotOwned {
                uid: uid.to_string(),
            });
        }
        if let Some(mut live) = self.sidecars.remove(&id) {
            sidecar::terminate(&mut live.child);
        }
        Ok(())
    }

    fn require(&self, uid: FiberUid, op: &str) -> Result<(), BrokerError> {
        if self.has_grant(uid, op) {
            Ok(())
        } else {
            Err(BrokerError::Denied {
                uid: uid.to_string(),
                op: op.to_owned(),
            })
        }
    }
}

fn parse_rg_json(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn validate_glob(pattern: &str) -> Result<(), BrokerError> {
    for component in Path::new(pattern).components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(BrokerError::InvalidGlob(pattern.to_owned()));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

impl Drop for Broker {
    fn drop(&mut self) {
        for (_, mut live) in self.sidecars.drain() {
            sidecar::terminate(&mut live.child);
        }
    }
}
