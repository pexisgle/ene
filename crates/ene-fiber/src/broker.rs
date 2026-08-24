use crate::fiber::FiberUid;
use crate::sidecar::{self, LiveSidecar, SidecarHealth, SidecarId, SidecarRequest};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Keeps base64 broker payloads inside the configured 1 MiB IPC frame.
const MAX_BROKER_FILE_BYTES: usize = 512 * 1024;

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
    #[error("invalid regular expression: {0}")]
    InvalidRegex(String),
    #[error("search engine is unavailable")]
    SearchEngineUnavailable,
    #[error("refusing to delete a symlink")]
    Symlink,
    #[error("directory is not empty")]
    NotEmpty,
    #[error("path is read-only")]
    ReadOnly,
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
    let resolved = confine_lexical(workspace, path, create_parent)?;
    if resolved.exists() {
        let canonical = resolved.canonicalize()?;
        let base = workspace.canonicalize()?;
        if !canonical.starts_with(&base) {
            return Err(BrokerError::PathEscape(canonical.display().to_string()));
        }
        return Ok(canonical);
    }
    Ok(resolved)
}

fn confine_lexical(
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

    pub fn revoke(&mut self, uid: FiberUid, op: &str) {
        let Some(ops) = self.grants.get_mut(&uid) else {
            return;
        };
        ops.remove(op);
        if ops.is_empty() {
            self.grants.remove(&uid);
        }
    }

    pub fn revoke_all(&mut self, uid: FiberUid) {
        self.grants.remove(&uid);
        self.release_owned_sidecars(uid);
    }

    pub fn release_owned_sidecars(&mut self, uid: FiberUid) {
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

    #[must_use]
    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn fs_read(&self, uid: FiberUid, path: &Path) -> Result<String, BrokerError> {
        self.require(uid, "fs.read")?;
        fs_read(&self.workspace, path)
    }

    pub fn fs_read_bytes(&self, uid: FiberUid, path: &Path) -> Result<Vec<u8>, BrokerError> {
        self.require(uid, "fs.read")?;
        fs_read_bytes(&self.workspace, path)
    }

    pub fn fs_write(&self, uid: FiberUid, path: &Path, text: &str) -> Result<(), BrokerError> {
        self.require(uid, "fs.write")?;
        fs_write(&self.workspace, path, text)
    }

    pub fn fs_write_bytes(
        &self,
        uid: FiberUid,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), BrokerError> {
        self.require(uid, "fs.write")?;
        fs_write_bytes(&self.workspace, path, bytes)
    }

    pub fn fs_list(&self, uid: FiberUid, path: &Path) -> Result<Vec<String>, BrokerError> {
        self.require(uid, "fs.list")?;
        let resolved = confine_path(&self.workspace, path, false)?;
        let mut names = Vec::new();
        for ent in std::fs::read_dir(resolved)? {
            names.push(ent?.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    pub fn fs_glob(&self, uid: FiberUid, pattern: &str) -> Result<Vec<String>, BrokerError> {
        self.require(uid, "fs.glob")?;
        if Path::new(pattern).is_absolute()
            || Path::new(pattern)
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(BrokerError::PathEscape(pattern.to_owned()));
        }
        let root = self.workspace.canonicalize()?;
        let mut paths = Vec::new();
        broker_walk_glob(&root, &root, pattern, &mut paths, 500)?;
        paths.sort();
        Ok(paths)
    }

    pub fn fs_delete(&self, uid: FiberUid, path: &Path) -> Result<(), BrokerError> {
        self.require(uid, "fs.delete")?;
        let lexical = confine_lexical(&self.workspace, path, false)?;
        let meta = std::fs::symlink_metadata(&lexical)?;
        if is_link_or_reparse(&meta) {
            return Err(BrokerError::Symlink);
        }
        let base = self.workspace.canonicalize()?;
        let resolved = lexical
            .canonicalize()
            .map_err(|_| BrokerError::PathEscape(lexical.display().to_string()))?;
        if !resolved.starts_with(&base) {
            return Err(BrokerError::PathEscape(resolved.display().to_string()));
        }
        if meta.permissions().readonly() {
            return Err(BrokerError::ReadOnly);
        }
        if meta.is_dir() {
            if std::fs::read_dir(&resolved)?.next().is_some() {
                return Err(BrokerError::NotEmpty);
            }
            std::fs::remove_dir(resolved)?;
        } else {
            std::fs::remove_file(resolved)?;
        }
        Ok(())
    }

    pub fn net_fetch(&self, uid: FiberUid, url: &str) -> Result<Value, BrokerError> {
        self.require(uid, "net.fetch")?;
        net_fetch(url)
    }

    /// JSON POST through the same grant and SSRF pipeline as [`Self::net_fetch`].
    ///
    /// # Errors
    ///
    /// Returns the underlying fetch error when a hop is denied or fails.
    pub fn net_post_json(
        &self,
        uid: FiberUid,
        url: &str,
        body: &Value,
        bearer: Option<&str>,
    ) -> Result<Value, BrokerError> {
        self.require(uid, "net.fetch")?;
        net_post_json(url, body, bearer)
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
        fs_search(
            &self.workspace,
            path,
            query,
            regex,
            case_insensitive,
            include,
            context_lines,
            count,
            max,
        )
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

pub(crate) fn fs_read(workspace: &Path, path: &Path) -> Result<String, BrokerError> {
    let resolved = confine_path(workspace, path, false)?;
    std::fs::read_to_string(resolved).map_err(BrokerError::from)
}

pub(crate) fn fs_read_bytes(workspace: &Path, path: &Path) -> Result<Vec<u8>, BrokerError> {
    let resolved = confine_path(workspace, path, false)?;
    let bytes = std::fs::read(resolved).map_err(BrokerError::from)?;
    if bytes.len() > MAX_BROKER_FILE_BYTES {
        return Err(BrokerError::Oversize);
    }
    Ok(bytes)
}

pub(crate) fn fs_write(workspace: &Path, path: &Path, text: &str) -> Result<(), BrokerError> {
    let resolved = confine_path(workspace, path, true)?;
    std::fs::write(resolved, text).map_err(BrokerError::from)
}

pub(crate) fn fs_write_bytes(
    workspace: &Path,
    path: &Path,
    bytes: &[u8],
) -> Result<(), BrokerError> {
    if bytes.len() > MAX_BROKER_FILE_BYTES {
        return Err(BrokerError::Oversize);
    }
    let resolved = confine_path(workspace, path, true)?;
    std::fs::write(resolved, bytes).map_err(BrokerError::from)
}

pub(crate) fn net_fetch(url: &str) -> Result<Value, BrokerError> {
    crate::net::get(url)
}

pub(crate) fn fs_search(
    workspace: &Path,
    path: &Path,
    query: &str,
    regex: bool,
    case_insensitive: bool,
    include: Option<&str>,
    context_lines: u32,
    count: bool,
    max: u32,
) -> Result<Value, BrokerError> {
    let root = confine_path(workspace, path, false)?;
    let include = match include {
        Some(pattern) => {
            validate_glob(pattern)?;
            Some(pattern)
        }
        None => None,
    };
    if regex {
        rg_regex::Regex::new(query).map_err(|err| BrokerError::InvalidRegex(err.to_string()))?;
    }
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
    if context_lines > 0 && !count {
        command.arg(format!("--context={context_lines}"));
    }

    let max = usize::try_from(max).unwrap_or(200).min(200);
    command
        .arg("--json")
        .arg("--")
        .arg(query)
        .arg(&root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = command.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            BrokerError::SearchEngineUnavailable
        } else {
            BrokerError::from(err)
        }
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

    if !output.status.success() && output.status.code() != Some(1) {
        return Err(BrokerError::Fetch(stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_rg_json(&stdout, max, count))
}

fn parse_rg_json(stdout: &str, max: usize, count_only: bool) -> Value {
    let mut matches = Vec::new();
    let mut counts = BTreeMap::<String, u64>::new();
    let mut total = 0_u64;
    for event in stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if event.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }
        let data = &event["data"];
        let path = data["path"]["text"].as_str().unwrap_or("").to_owned();
        total = total.saturating_add(1);
        if count_only {
            *counts.entry(path).or_default() += 1;
            continue;
        }
        if matches.len() >= max {
            continue;
        }
        let line = data["line_number"].as_u64().unwrap_or(0);
        let text = data["lines"]["text"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        matches.push(json!({
            "path": path,
            "line": line,
            "text": text,
        }));
    }
    if count_only {
        let files: Vec<Value> = counts
            .into_iter()
            .map(|(path, count)| json!({ "path": path, "count": count }))
            .collect();
        return json!({ "files": files, "total": total });
    }
    Value::Array(matches)
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

fn broker_walk_glob(
    root: &Path,
    dir: &Path,
    pattern: &str,
    out: &mut Vec<String>,
    max: usize,
) -> Result<(), BrokerError> {
    if out.len() >= max {
        return Ok(());
    }
    for ent in std::fs::read_dir(dir)? {
        if out.len() >= max {
            break;
        }
        let ent = ent?;
        let path = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == ".ene" || name.starts_with('.') {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if is_link_or_reparse(&meta) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| BrokerError::PathEscape(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if broker_glob_match(pattern, &rel) {
            out.push(rel);
        }
        if meta.is_dir() {
            broker_walk_glob(root, &path, pattern, out, max)?;
        }
    }
    Ok(())
}

fn broker_glob_match(pattern: &str, rel: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path: Vec<&str> = rel.split('/').filter(|part| !part.is_empty()).collect();
    broker_glob_components(&pat, &path)
}

fn broker_glob_components(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first().copied(), path.first().copied()) {
        (None, None) => true,
        (Some("**"), _) => {
            broker_glob_components(&pat[1..], path)
                || (!path.is_empty() && broker_glob_components(pat, &path[1..]))
        }
        (Some(seg), Some(name)) if broker_glob_segment(seg, name) => {
            broker_glob_components(&pat[1..], &path[1..])
        }
        _ => false,
    }
}

fn broker_glob_segment(pattern: &str, name: &str) -> bool {
    broker_glob_stars(pattern.as_bytes(), name.as_bytes())
}

fn broker_glob_stars(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first().copied(), name.first().copied()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            broker_glob_stars(&pattern[1..], name)
                || (!name.is_empty() && broker_glob_stars(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => broker_glob_stars(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) if p == n => broker_glob_stars(&pattern[1..], &name[1..]),
        _ => false,
    }
}

fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        for (_, mut live) in self.sidecars.drain() {
            sidecar::terminate(&mut live.child);
        }
    }
}

#[cfg(test)]
mod confine_tests {
    use super::{Broker, BrokerError, confine_lexical};
    use crate::fiber::FiberUid;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn fs_delete_rejects_workspace_symlink_to_in_workspace_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("target.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", dir.path().join("link.txt")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("target.txt", dir.path().join("link.txt")).unwrap();
        let mut broker = Broker::new(dir.path().to_path_buf());
        let uid = FiberUid::new();
        broker.grant(uid, "fs.delete");
        assert!(matches!(
            broker.fs_delete(uid, Path::new("link.txt")),
            Err(BrokerError::Symlink)
        ));
        assert!(dir.path().join("target.txt").exists());
    }

    #[test]
    fn confine_lexical_does_not_dereference_final_component() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("target.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", dir.path().join("link.txt")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("target.txt", dir.path().join("link.txt")).unwrap();
        let lexical = confine_lexical(dir.path(), Path::new("link.txt"), false).unwrap();
        assert!(
            std::fs::symlink_metadata(&lexical)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
