use crate::fiber::FiberUid;
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
}

/// One grant tracked as a fiber effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub uid: FiberUid,
    pub op: String,
}

/// Minimal fs/net broker. Grants are per-fiber; undeclared ops are denied (I-48).
#[derive(Debug)]
pub struct Broker {
    grants: HashMap<FiberUid, HashSet<String>>,
    workspace: PathBuf,
}

impl Broker {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            grants: HashMap::new(),
            workspace,
        }
    }

    pub fn grant(&mut self, uid: FiberUid, op: impl Into<String>) {
        self.grants.entry(uid).or_default().insert(op.into());
    }

    pub fn revoke_all(&mut self, uid: FiberUid) {
        self.grants.remove(&uid);
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
