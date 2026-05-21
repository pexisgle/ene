use serde::{Deserialize, Serialize};

/// SandboxConfig のシリアライズ可能データ型（POD）
///
/// バリデーションロジックは含まず、ene-tools/fs::Sandbox で構築・検証する。
/// core → host 間のIPC通信で使用される。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxConfigData {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,
    pub writable_directories: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub shell_timeout_ms: u64,
    pub max_shell_output_bytes: usize,
    pub max_shell_output_lines: usize,
}
