use std::path::{Path, PathBuf};
use crate::error::AiCoreError;
use crate::sandbox::permission::{PermissionGate, DestructiveAction};

pub mod permission;

/// サンドボックス設定 — 許可ディレクトリと制限
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
        }
    }
}

impl SandboxConfig {
    /// パスを正規化して許可チェック
    pub fn resolve_and_check(&self, path: &Path, require_writable: bool) -> Result<PathBuf, AiCoreError> {
        // 存在しないファイルの場合、親ディレクトリをチェック
        let check_path = if path.exists() {
            std::fs::canonicalize(path).map_err(|e| AiCoreError::SandboxViolation(format!("Cannot resolve path: {e}")))?
        } else {
            // 親ディレクトリが存在するか確認
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
            };
            if let Some(parent) = abs.parent() {
                if parent.exists() {
                    let canonical_parent = std::fs::canonicalize(parent)
                        .map_err(|e| AiCoreError::SandboxViolation(format!("Cannot resolve parent directory: {e}")))?;
                    canonical_parent.join(abs.file_name().unwrap_or_default())
                } else {
                    return Err(AiCoreError::SandboxViolation(format!(
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

        Err(AiCoreError::SandboxViolation(format!(
            "Path not allowed: {}. Allowed dirs: {:?}",
            check_path.display(),
            allowed
        )))
    }

    /// コマンドがブロックリストにマッチするかチェック
    pub fn is_command_blocked(&self, command: &str) -> Result<(), AiCoreError> {
        for pattern in &self.blocked_commands {
            let re = regex::Regex::new(pattern)
                .map_err(|e| AiCoreError::ConfigError(format!("Invalid blocked command pattern: {e}")))?;
            if re.is_match(command) {
                return Err(AiCoreError::CommandBlocked(format!(
                    "Command matches blocked pattern: {}",
                    pattern
                )));
            }
        }
        Ok(())
    }

    /// 破壊的操作のパーミッションをチェック
    pub fn check_permission(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), AiCoreError> {
        let gate = PermissionGate::default_with_sandbox(self);
        match gate.check_destructive(action, target, description) {
            Ok(()) => Ok(()),
            Err(req) => Err(AiCoreError::PermissionDenied(format!(
                "{:?} on {} requires approval: {}",
                action,
                req.description,
                target
            ))),
        }
    }
}
