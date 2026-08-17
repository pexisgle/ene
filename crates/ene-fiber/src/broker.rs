use crate::fiber::FiberUid;
use crate::sidecar::{self, LiveSidecar, SidecarHealth, SidecarId, SidecarRequest};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
}

/// One grant tracked as a fiber effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub uid: FiberUid,
    pub op: String,
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
        let resolved = self.confine(path)?;
        std::fs::read_to_string(resolved).map_err(BrokerError::from)
    }

    pub fn fs_write(&self, uid: FiberUid, path: &Path, text: &str) -> Result<(), BrokerError> {
        self.require(uid, "fs.write")?;
        let resolved = self.confine(path)?;
        std::fs::write(resolved, text).map_err(BrokerError::from)
    }

    pub fn net_fetch(&self, uid: FiberUid, _url: &str) -> Result<Value, BrokerError> {
        self.require(uid, "net.fetch")?;
        Err(BrokerError::Denied {
            uid: uid.to_string(),
            op: "net.fetch".to_owned(),
        })
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

    fn confine(&self, path: &Path) -> Result<PathBuf, BrokerError> {
        let requested = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };
        let base = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.clone());
        let resolved = requested.canonicalize().unwrap_or(requested);
        if resolved.starts_with(&base) {
            Ok(resolved)
        } else {
            Err(BrokerError::PathEscape(resolved.display().to_string()))
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        for (_, mut live) in self.sidecars.drain() {
            sidecar::terminate(&mut live.child);
        }
    }
}
