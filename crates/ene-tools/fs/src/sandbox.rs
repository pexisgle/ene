use crate::permission::{DestructiveAction, PermissionGate};
use ene_tool_proto::ToolError;
use std::path::{Path, PathBuf};

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
    /// パスを正規化して許可チェック
    pub fn resolve_and_check(
        &self,
        path: &Path,
        require_writable: bool,
    ) -> Result<PathBuf, ToolError> {
        // 存在しないファイルの場合、親ディレクトリをチェック
        let check_path = if path.exists() {
            std::fs::canonicalize(path).map_err(|e| ToolError::SandboxViolation {
                message: format!("Cannot resolve path: {e}"),
            })?
        } else {
            // 親ディレクトリが存在するか確認
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

    /// コマンドがブロックリストにマッチするかチェック
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

    /// 破壊的操作のパーミッションをチェック
    pub fn check_permission(
        &self,
        action: DestructiveAction,
        target: &str,
        description: &str,
    ) -> Result<(), ToolError> {
        let gate = PermissionGate::default_with_sandbox(self);
        match gate.check_destructive(action, target, description) {
            Ok(()) => Ok(()),
            Err(req) => Err(ToolError::PermissionDenied {
                message: format!(
                    "{:?} on {} requires approval: {}",
                    action, req.description, target
                ),
            }),
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

/// SandboxConfig + UndoManager + session_id の統合型
///
/// ツール実装者は Sandbox だけを意識すればよい。
/// アクセス制御（check_readable / check_writable / check_command）と
/// 操作追跡（track_xxx）および Undo 実行を一つに束ねる。
pub struct Sandbox {
    config: SandboxConfig,
    undo: crate::undo_manager::UndoManager,
    session_id: std::sync::RwLock<String>,
}

impl Sandbox {
    pub fn new(config: SandboxConfig) -> Self {
        let undo = crate::undo_manager::UndoManager::new();
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
        }
    }

    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    pub fn undo_manager(&self) -> &crate::undo_manager::UndoManager {
        &self.undo
    }

    pub fn session_id(&self) -> String {
        self.session_id.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_session_id(&self, id: &str) {
        *self.session_id.write().unwrap_or_else(|e| e.into_inner()) = id.to_string();
    }

    /// 読み取りパス検証
    pub fn check_readable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, false)
    }

    /// 書き込みパス検証
    pub fn check_writable(&self, path: &Path) -> Result<PathBuf, ToolError> {
        self.config.resolve_and_check(path, true)
    }

    /// コマンド検証
    pub fn check_command(&self, cmd: &str) -> Result<(), ToolError> {
        self.config.is_command_blocked(cmd)
    }

    /// 既存ファイルの上書きを記録
    pub fn track_overwrite(&self, original_content: Option<Vec<u8>>, path: &Path) {
        self.undo.push_restore_file(
            &self.session_id(),
            "filesystem",
            path.to_path_buf(),
            original_content,
        );
    }

    /// 新規ファイル作成を記録
    pub fn track_creation(&self, path: &Path) {
        self.undo
            .push_delete_created_file(&self.session_id(), "filesystem", path.to_path_buf());
    }

    /// ファイル削除を記録
    pub fn track_deletion(&self, original_content: Option<Vec<u8>>, path: &Path) {
        self.undo.push_restore_file(
            &self.session_id(),
            "filesystem",
            path.to_path_buf(),
            original_content,
        );
    }

    /// パッチ適用を記録（複数ファイル操作を1つのUndoエントリに）
    pub fn track_patch(&self, operations: Vec<crate::undo_manager::UndoOperation>) {
        self.undo
            .push(&self.session_id(), crate::undo_manager::UndoEntry::new("patch", operations));
    }

    /// 直前の操作を取り消し
    pub async fn undo_last(&self) -> Result<String, ToolError> {
        let logs = self
            .undo
            .undo(&self.session_id())
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Undo failed: {e}"),
            })?;
        Ok(logs.join("\n"))
    }
}
