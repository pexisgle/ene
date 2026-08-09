use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::category::ApprovalCategory;
use crate::mode::ResolvedMode;
use crate::policy::ResolutionReason;

/// One audit record: what was requested, which rule decided, and the outcome.
///
/// Secrets, request bodies, and user file contents are never stored here —
/// `target` is the audit-safe description from the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Unix millisecond timestamp of the resolution.
    pub ts_ms: u64,
    /// Plugin that made the request.
    pub plugin: String,
    /// Digest of the plugin's signed manifest, when one was loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    /// Category of the request.
    pub category: ApprovalCategory,
    /// Audit-safe target description (origin, path, artifact, key name…).
    pub target: String,
    /// Which layer decided (`emergency_stop`, `plugin_override`,
    /// `global_policy`, `default_ask`).
    pub reason: String,
    /// The rule text that applied.
    pub rule: String,
    /// The effective decision.
    pub decision: ResolvedMode,
}

/// Append-only JSON-lines audit log.
///
/// Every resolution — automatic or interactive — is recorded here so an
/// operator can reconstruct which rule allowed or denied which request.
#[derive(Debug)]
pub struct AuditLog {
    path: PathBuf,
    writer: Mutex<Option<BufWriter<File>>>,
}

impl AuditLog {
    /// Opens (creating if needed) the audit log at `path`.
    ///
    /// The log is opened lazily on first write; a missing parent directory
    /// fails the first [`record`](Self::record) call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writer: Mutex::new(None),
        }
    }

    /// The log file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one entry as a JSON line.
    pub fn record(&self, entry: &AuditLogEntry) -> std::io::Result<()> {
        let mut guard = self.writer.lock();
        if guard.is_none() {
            if let Some(parent) = self.path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            *guard = Some(BufWriter::new(file));
        }
        let Some(writer) = guard.as_mut() else {
            return Err(std::io::Error::other("audit log writer not initialized"));
        };
        serde_json::to_writer(&mut *writer, entry).map_err(std::io::Error::other)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}

impl AuditLogEntry {
    /// Builds an entry from a resolution.
    #[must_use]
    pub fn new(
        ts_ms: u64,
        plugin: String,
        manifest_digest: Option<String>,
        category: ApprovalCategory,
        target: String,
        reason: ResolutionReason,
        rule: String,
        decision: ResolvedMode,
    ) -> Self {
        Self {
            ts_ms,
            plugin,
            manifest_digest,
            category,
            target,
            reason: reason.label().to_string(),
            rule,
            decision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::ApprovalCategory;
    use crate::mode::ResolvedMode;
    use crate::policy::ResolutionReason;

    #[test]
    fn audit_log_appends_json_lines_and_reopens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("audit.jsonl");
        let log = AuditLog::new(&path);
        let entry = AuditLogEntry::new(
            1_700_000_000_000,
            "fs".to_string(),
            Some("digest".to_string()),
            ApprovalCategory::FsDelete,
            "/home/user/notes.txt".to_string(),
            ResolutionReason::GlobalPolicy,
            "global policy denies this category".to_string(),
            ResolvedMode::Deny,
        );
        log.record(&entry).expect("record");

        let line = std::fs::read_to_string(&path).expect("read log");
        assert!(line.ends_with('\n'));
        let parsed: AuditLogEntry = serde_json::from_str(line.trim_end()).expect("parse");
        assert_eq!(parsed, entry);
        assert_eq!(parsed.category, ApprovalCategory::FsDelete);
        assert_eq!(parsed.reason, "global_policy");
        assert_eq!(parsed.decision, ResolvedMode::Deny);

        // A second writer on the same file must append, not truncate.
        let log2 = AuditLog::new(&path);
        log2.record(&entry).expect("record second");
        let lines = std::fs::read_to_string(&path).expect("read log");
        assert_eq!(lines.lines().count(), 2);
    }
}
