use crate::undo::UndoManager;
use crate::utils::permission::{DestructiveAction, PermissionGate};
use ene_plugin_proto::ToolError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Pre-compiled blocklist regexes paired with their source pattern.
///
/// Stored as a `Result` so that regex compilation failures are
/// preserved and surfaced as errors on first use rather than
/// silently dropping patterns (which would cause index mismatch).
type CompiledBlocklist = Result<Vec<(regex::Regex, String)>, String>;

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub allowed_directories: Vec<PathBuf>,
    pub writable_directories: Vec<PathBuf>,
    pub blocked_commands: Vec<String>,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub shell_timeout_ms: u64,
    pub max_shell_output_bytes: usize,
    pub max_shell_output_lines: usize,
    pub db_socket: Option<PathBuf>,
    pub db_auth_token: Option<String>,
    /// Cache of pre-compiled `blocked_commands` regexes. The struct
    /// is `Clone`d, so the cache is held in an `Arc` to keep the
    /// cached regexes shared across clones. Not part of the
    /// `SandboxConfigData` wire format; populated by `Default` and
    /// the `From<SandboxConfigData>` impl.
    pub compiled_blocklist: std::sync::Arc<std::sync::OnceLock<CompiledBlocklist>>,
    /// Thread-safe dynamic allowlist patterns shared with Sandbox.
    pub allowed_patterns:
        std::sync::Arc<std::sync::RwLock<std::collections::HashSet<(String, String)>>>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let canonical_cwd = if cwd.exists() {
            std::fs::canonicalize(&cwd).unwrap_or(cwd)
        } else {
            cwd
        };
        Self {
            enabled: true,
            allowed_directories: vec![canonical_cwd.clone()],
            writable_directories: vec![canonical_cwd],
            blocked_commands: vec![
                r"rm\s+-rf\s+/".to_string(),
                r"dd\s+if=".to_string(),
                r"mkfs".to_string(),
                r":\s*\{\s*\|\s*&\s*;\s*\}".to_string(),
                r"sudo\s+".to_string(),
            ],
            max_read_bytes: 50 * 1024,
            max_write_bytes: 1024 * 1024,
            shell_timeout_ms: 120_000,
            max_shell_output_bytes: 50 * 1024,
            max_shell_output_lines: 2000,
            db_socket: None,
            db_auth_token: None,
            compiled_blocklist: std::sync::Arc::new(std::sync::OnceLock::new()),
            allowed_patterns: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
        }
    }
}

impl SandboxConfig {
    pub fn resolve_and_check(
        &self,
        path: &Path,
        require_writable: bool,
    ) -> Result<PathBuf, ToolError> {
        let check_path = if path.exists() {
            std::fs::canonicalize(path)
                .map_err(|e| ToolError::sandbox_violation(format!("Cannot resolve path: {e}")))?
        } else {
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(path)
            };
            if let Some(parent) = abs.parent() {
                if parent.exists() {
                    let canonical_parent = std::fs::canonicalize(parent).map_err(|e| {
                        ToolError::sandbox_violation(format!(
                            "Cannot resolve parent directory: {e}"
                        ))
                    })?;
                    canonical_parent.join(abs.file_name().unwrap_or_default())
                } else {
                    return Err(ToolError::sandbox_violation(format!(
                        "Parent directory does not exist: {}",
                        parent.display()
                    )));
                }
            } else {
                abs
            }
        };

        let allowed = if require_writable {
            &self.writable_directories
        } else {
            &self.allowed_directories
        };

        for dir in allowed {
            let canonical_dir = if dir.exists() {
                std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone())
            } else {
                dir.clone()
            };
            if check_path.starts_with(&canonical_dir) {
                return Ok(check_path);
            }
        }

        if let Ok(guard) = self.allowed_patterns.read() {
            for (allowed_action, allowed_target) in guard.iter() {
                let action_ok = if require_writable {
                    allowed_action == "FileOverwrite" || allowed_action == "FileDelete"
                } else {
                    true
                };
                if action_ok {
                    let prefix = std::path::Path::new(allowed_target);
                    let canonical_prefix = if prefix.exists() {
                        std::fs::canonicalize(prefix).unwrap_or_else(|_| prefix.to_path_buf())
                    } else if let Some(parent) = prefix.parent() {
                        if parent.exists() {
                            if let Ok(p) = std::fs::canonicalize(parent) {
                                p.join(prefix.file_name().unwrap_or_default())
                            } else {
                                prefix.to_path_buf()
                            }
                        } else {
                            prefix.to_path_buf()
                        }
                    } else {
                        prefix.to_path_buf()
                    };
                    if check_path.starts_with(&canonical_prefix) || check_path.starts_with(prefix) {
                        return Ok(check_path);
                    }
                }
            }
        }

        Err(ToolError::sandbox_violation(format!(
            "Path not allowed: {}. Allowed dirs: {:?}",
            check_path.display(),
            allowed
        )))
    }

    /// Checks whether a command matches the blocklist.
    ///
    /// The blocklist regexes are compiled once on first call and
    /// cached in an `Arc<OnceLock>` so the compiled set is shared
    /// across `Clone`s of this `SandboxConfig`. After the first
    /// call this is an O(n) scan over pre-compiled regexes rather
    /// than a recompile of every pattern on every invocation.
    pub fn is_command_blocked(&self, command: &str) -> Result<(), ToolError> {
        let compiled = self.compiled_blocklist.get_or_init(|| {
            self.blocked_commands
                .iter()
                .map(|p| {
                    regex::Regex::new(p)
                        .map(|re| (re, p.clone()))
                        .map_err(|e| format!("Failed to compile blocklist pattern '{p}': {e}"))
                })
                .collect()
        });

        let compiled = compiled
            .as_ref()
            .map_err(|e| ToolError::internal(e.clone()))?;

        for (re, pattern) in compiled {
            if re.is_match(command) {
                return Err(ToolError::sandbox_violation(format!(
                    "Command matches blocked pattern: {pattern}"
                )));
            }
        }
        Ok(())
    }

    pub fn check_permission(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), ToolError> {
        let gate = PermissionGate::default_with_sandbox(self);
        match gate.check_destructive(action, target, description) {
            Ok(()) => Ok(()),
            Err(req) => {
                let action_str = match req.level {
                    crate::utils::permission::PermissionLevel::RequiresApproval {
                        action, ..
                    } => action,
                    _ => format!("{action:?}"),
                };
                Err(ToolError::PermissionRequired {
                    request_id: req.id.to_string(),
                    action: action_str,
                    target: target.to_string(),
                    description: req.description,
                })
            }
        }
    }
}

impl From<ene_plugin_proto::SandboxConfigData> for SandboxConfig {
    fn from(mut data: ene_plugin_proto::SandboxConfigData) -> Self {
        data.sanitize();
        let canonicalize_dirs = |dirs: Vec<String>| -> Vec<PathBuf> {
            dirs.into_iter()
                .map(|s| {
                    let p = PathBuf::from(s);
                    if p.exists() {
                        std::fs::canonicalize(&p).unwrap_or(p)
                    } else {
                        p
                    }
                })
                .collect()
        };

        Self {
            enabled: data.enabled,
            allowed_directories: canonicalize_dirs(data.allowed_directories),
            writable_directories: canonicalize_dirs(data.writable_directories),
            blocked_commands: data.blocked_commands,
            max_read_bytes: data.max_read_bytes,
            max_write_bytes: data.max_write_bytes,
            shell_timeout_ms: data.shell_timeout_ms,
            max_shell_output_bytes: data.max_shell_output_bytes,
            max_shell_output_lines: data.max_shell_output_lines,
            db_socket: data.db_socket.map(PathBuf::from),
            db_auth_token: data.db_auth_token,
            compiled_blocklist: std::sync::Arc::new(std::sync::OnceLock::new()),
            allowed_patterns: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
        }
    }
}

/// Integrated type combining `SandboxConfig` + shared `UndoManager` + `session_id`.
///
/// Tool implementers only need to be aware of Sandbox.
/// Access control (`check_readable` / `check_writable` / `check_command`) and
/// operation tracking (`track_xxx`) and Undo execution are bundled together.
pub struct Sandbox {
    config: SandboxConfig,
    undo_manager: std::sync::Mutex<Option<Arc<UndoManager>>>,
    session_id: std::sync::RwLock<String>,
    approved_requests: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    allowed_patterns:
        std::sync::Arc<std::sync::RwLock<std::collections::HashSet<(String, String)>>>,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Self {
        let allowed_patterns = config.allowed_patterns.clone();
        Self {
            config,
            undo_manager: std::sync::Mutex::new(None),
            session_id: std::sync::RwLock::new(String::new()),
            approved_requests: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            allowed_patterns,
        }
    }

    async fn ensure_undo_manager(&self) -> Result<Arc<UndoManager>, ToolError> {
        {
            let guard = self
                .undo_manager
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(mgr) = guard.as_ref() {
                return Ok(mgr.clone());
            }
        }

        let socket_path = self
            .config
            .db_socket
            .as_deref()
            .ok_or_else(|| ToolError::internal("DB socket path not configured".to_string()))?;

        let session_id = self.session_id();
        let mgr = UndoManager::new(
            socket_path,
            session_id,
            self.config.db_auth_token.as_deref(),
        )
        .await
        .map_err(|e| ToolError::execution_failed(format!("Failed to connect to DB: {e}")))?;

        let mgr = Arc::new(mgr);
        let mut guard = self
            .undo_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = guard.clone() {
            drop(guard);
            Ok(existing)
        } else {
            *guard = Some(mgr.clone());
            drop(guard);
            Ok(mgr)
        }
    }

    pub const fn config(&self) -> &SandboxConfig {
        &self.config
    }

    pub fn session_id(&self) -> String {
        self.session_id
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_session_id(&self, id: &str) {
        *self
            .session_id
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = id.to_string();

        let mut guard = self
            .undo_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
    }

    pub fn approve_request(&self, request_id: &str) {
        if let Ok(mut guard) = self.approved_requests.write() {
            guard.insert(request_id.to_string());
        }
    }

    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.insert((action.to_string(), target_pattern.to_string()));
        }
    }

    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.remove(&(action.to_string(), target_pattern.to_string()));
        }
    }

    pub fn check_permission(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), ToolError> {
        let action_str = format!("{action:?}");

        if let Ok(guard) = self.allowed_patterns.read() {
            for (allowed_action, allowed_target) in guard.iter() {
                if allowed_action != &action_str {
                    continue;
                }
                let path = std::path::Path::new(target);
                let prefix = std::path::Path::new(allowed_target);
                if path.starts_with(prefix) {
                    return Ok(());
                }
            }
        }

        match self.config.check_permission(action, target, description) {
            Ok(()) => Ok(()),
            Err(ToolError::PermissionRequired {
                request_id,
                action: act,
                target: tgt,
                description: desc,
            }) => {
                if let Ok(guard) = self.approved_requests.read()
                    && guard.contains(&request_id)
                {
                    return Ok(());
                }
                Err(ToolError::PermissionRequired {
                    request_id,
                    action: act,
                    target: tgt,
                    description: desc,
                })
            }
            Err(other) => Err(other),
        }
    }

    pub fn check_readable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, false)
    }

    pub fn check_writable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, true)
    }

    pub fn check_command(&self, cmd: &str) -> Result<(), ToolError> {
        self.config.is_command_blocked(cmd)
    }

    pub async fn track_overwrite(&self, path: &Path, original_content: Option<Vec<u8>>) {
        let mgr = match self.ensure_undo_manager().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[Sandbox] Failed to get undo manager: {e}");
                return;
            }
        };
        mgr.push_restore_file("filesystem", path.display().to_string(), original_content)
            .await;
    }

    pub async fn track_creation(&self, path: &Path) {
        let mgr = match self.ensure_undo_manager().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[Sandbox] Failed to get undo manager: {e}");
                return;
            }
        };
        mgr.push_delete_created_file("filesystem", path.display().to_string())
            .await;
    }

    pub async fn track_deletion(&self, path: &Path, original_content: Option<Vec<u8>>) {
        let mgr = match self.ensure_undo_manager().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[Sandbox] Failed to get undo manager: {e}");
                return;
            }
        };
        mgr.push_restore_file("filesystem", path.display().to_string(), original_content)
            .await;
    }

    pub async fn track_patch(&self, operations: Vec<crate::undo::UndoOperation>) {
        let mgr = match self.ensure_undo_manager().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[Sandbox] Failed to get undo manager: {e}");
                return;
            }
        };
        let entry = crate::undo::UndoEntry::new("patch", operations);
        if let Err(e) = mgr.push(entry).await {
            tracing::warn!("[Sandbox] Failed to push patch undo: {e}");
        }
    }

    pub async fn undo_last(&self) -> Result<String, ToolError> {
        let mgr = self.ensure_undo_manager().await?;
        let entry = mgr
            .peek()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Undo failed: {e}")))?;

        let Some(entry) = entry else {
            return Err(ToolError::execution_failed(
                "No operations to undo".to_string(),
            ));
        };

        let mut logs: Vec<String> = Vec::new();
        let mut apply_error: Option<ToolError> = None;
        for op in &entry.operations {
            match op {
                crate::undo::UndoOperation::RestoreFile {
                    path,
                    original_content,
                } => {
                    let result = if let Some(content) = original_content {
                        match tokio::fs::write(path, content).await {
                            Ok(()) => {
                                logs.push(format!("Restored: {path}"));
                                Ok(())
                            }
                            Err(e) => Err(ToolError::execution_failed(format!(
                                "Failed to restore file {path}: {e}"
                            ))),
                        }
                    } else {
                        match tokio::fs::remove_file(path).await {
                            Ok(()) => {
                                logs.push(format!("Removed (was not tracked): {path}"));
                                Ok(())
                            }
                            Err(e) => Err(ToolError::execution_failed(format!(
                                "Failed to remove file {path}: {e}"
                            ))),
                        }
                    };
                    if let Err(e) = result {
                        apply_error = Some(e);
                        break;
                    }
                }
                crate::undo::UndoOperation::DeleteCreatedFile { path } => {
                    match tokio::fs::remove_file(path).await {
                        Ok(()) => logs.push(format!("Deleted: {path}")),
                        Err(e) => {
                            apply_error = Some(ToolError::execution_failed(format!(
                                "Failed to delete created file {path}: {e}"
                            )));
                            break;
                        }
                    }
                }
            }
        }

        if let Some(e) = apply_error {
            return Err(e);
        }

        mgr.discard(&entry.id).await.map_err(|e| {
            ToolError::execution_failed(format!("Undo applied but failed to discard entry: {e}"))
        })?;

        Ok(logs.join("\n"))
    }
}
