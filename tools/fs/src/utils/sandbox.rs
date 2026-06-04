use crate::utils::permission::{DestructiveAction, PermissionGate};
use ene_tool_proto::ToolError;
use std::path::{Path, PathBuf};

/// Sandbox settings — allowed directories and restrictions
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
    pub undo_db_path: Option<PathBuf>,
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
            max_read_bytes: 50 * 1024,         // 50KB
            max_write_bytes: 1024 * 1024,      // 1MB
            shell_timeout_ms: 120_000,         // 2 minutes
            max_shell_output_bytes: 50 * 1024, // 50KB
            max_shell_output_lines: 2000,
            undo_db_path: None,
        }
    }
}

impl SandboxConfig {
    /// Normalizes the path and checks permissions
    pub fn resolve_and_check(
        &self,
        path: &Path,
        require_writable: bool,
    ) -> Result<PathBuf, ToolError> {
        // For non-existent files, check the parent directory
        let check_path = if path.exists() {
            std::fs::canonicalize(path).map_err(|e| ToolError::SandboxViolation {
                message: format!("Cannot resolve path: {e}"),
            })?
        } else {
            // Verify the parent directory exists
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(path)
            };
            if let Some(parent) = abs.parent() {
                if parent.exists() {
                    let canonical_parent =
                        std::fs::canonicalize(parent).map_err(|e| ToolError::SandboxViolation {
                            message: format!("Cannot resolve parent directory: {e}"),
                        })?;
                    canonical_parent.join(abs.file_name().unwrap_or_default())
                } else {
                    return Err(ToolError::SandboxViolation {
                        message: format!("Parent directory does not exist: {}", parent.display()),
                    });
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

        Err(ToolError::SandboxViolation {
            message: format!(
                "Path not allowed: {}. Allowed dirs: {:?}",
                check_path.display(),
                allowed
            ),
        })
    }

    /// Checks whether a command matches the blocklist
    pub fn is_command_blocked(&self, command: &str) -> Result<(), ToolError> {
        for pattern in &self.blocked_commands {
            let re = regex::Regex::new(pattern).map_err(|e| ToolError::Internal {
                message: format!("Invalid blocked command pattern: {e}"),
            })?;
            if re.is_match(command) {
                return Err(ToolError::SandboxViolation {
                    message: format!("Command matches blocked pattern: {}", pattern),
                });
            }
        }
        Ok(())
    }

    /// Checks permissions for destructive operations
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
                    _ => format!("{:?}", action),
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

impl From<ene_tool_proto::SandboxConfigData> for SandboxConfig {
    fn from(data: ene_tool_proto::SandboxConfigData) -> Self {
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
            undo_db_path: data.undo_db_path.map(PathBuf::from),
        }
    }
}

/// Integrated type combining SandboxConfig + UndoManager + session_id
///
/// Tool implementers only need to be aware of Sandbox.
/// Access control (check_readable / check_writable / check_command) and
/// operation tracking (track_xxx) and Undo execution are bundled together
pub struct Sandbox {
    config: SandboxConfig,
    undo: crate::utils::undo_manager::UndoManager,
    session_id: std::sync::RwLock<String>,
    approved_requests: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    allowed_patterns:
        std::sync::Arc<std::sync::RwLock<std::collections::HashSet<(String, String)>>>,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Self {
        let undo = crate::utils::undo_manager::UndoManager::new();
        if let Some(ref path) = config.undo_db_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = undo.set_db_path(path) {
                eprintln!("[Sandbox] Failed to open undo DB: {e}");
            }
        }
        Self {
            config,
            undo,
            session_id: std::sync::RwLock::new(String::new()),
            approved_requests: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
            allowed_patterns: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashSet::new(),
            )),
        }
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    pub fn undo_manager(&self) -> &crate::utils::undo_manager::UndoManager {
        &self.undo
    }

    pub fn session_id(&self) -> String {
        self.session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_session_id(&self, id: &str) {
        *self.session_id.write().unwrap_or_else(|e| e.into_inner()) = id.to_string();
    }

    /// Adds an approved request ID
    pub fn approve_request(&self, request_id: &str) {
        if let Ok(mut guard) = self.approved_requests.write() {
            guard.insert(request_id.to_string());
        }
    }

    /// Adds an action + target pattern combination
    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.insert((action.to_string(), target_pattern.to_string()));
        }
    }

    /// Permission check for destructive operations
    pub fn check_permission(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), ToolError> {
        let action_str = format!("{:?}", action);

        // 1. Session-level allow pattern check
        if let Ok(guard) = self.allowed_patterns.read() {
            for (allowed_action, allowed_target) in guard.iter() {
                if allowed_action == &action_str && target.contains(allowed_target) {
                    return Ok(());
                }
            }
        }

        // 2. Base permission config check
        match self.config.check_permission(action, target, description) {
            Ok(()) => Ok(()),
            Err(ToolError::PermissionRequired {
                request_id,
                action: act,
                target: tgt,
                description: desc,
            }) => {
                // Check if this specific request_id has been approved (Allow Once)
                if let Ok(guard) = self.approved_requests.read() {
                    if guard.contains(&request_id) {
                        return Ok(());
                    }
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

    /// Read path validation
    pub fn check_readable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, false)
    }

    /// Write path validation
    pub fn check_writable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, true)
    }

    /// Command validation
    pub fn check_command(&self, cmd: &str) -> Result<(), ToolError> {
        self.config.is_command_blocked(cmd)
    }

    /// Records overwrite of an existing file
    pub fn track_overwrite(&self, original_content: Option<Vec<u8>>, path: &Path) {
        self.undo.push_restore_file(
            &self.session_id(),
            "filesystem",
            path.to_path_buf(),
            original_content,
        );
    }

    /// Records creation of a new file
    pub fn track_creation(&self, path: &Path) {
        self.undo
            .push_delete_created_file(&self.session_id(), "filesystem", path.to_path_buf());
    }

    /// Records file deletion
    pub fn track_deletion(&self, original_content: Option<Vec<u8>>, path: &Path) {
        self.undo.push_restore_file(
            &self.session_id(),
            "filesystem",
            path.to_path_buf(),
            original_content,
        );
    }

    /// Records a patch application (multiple file operations in one Undo entry)
    pub fn track_patch(&self, operations: Vec<crate::utils::undo_manager::UndoOperation>) {
        self.undo.push(
            &self.session_id(),
            crate::utils::undo_manager::UndoEntry::new("patch", operations),
        );
    }

    /// Undoes the most recent operation
    pub async fn undo_last(&self) -> Result<String, ToolError> {
        let logs =
            self.undo
                .undo(&self.session_id())
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Undo failed: {e}"),
                })?;
        Ok(logs.join("\n"))
    }
}
