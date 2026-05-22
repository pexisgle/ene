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
    pub undo_db_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_config_data_serde_roundtrip() {
        let config = SandboxConfigData {
            enabled: true,
            allowed_directories: vec!["/home".into(), "/tmp".into()],
            writable_directories: vec!["/tmp".into()],
            blocked_commands: vec!["rm -rf".into()],
            max_read_bytes: 1024 * 1024,
            max_write_bytes: 512 * 1024,
            shell_timeout_ms: 30_000,
            max_shell_output_bytes: 1_000_000,
            max_shell_output_lines: 5000,
            undo_db_path: Some("/tmp/undo.db".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deser: SandboxConfigData = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deser);
    }

    #[test]
    fn sandbox_config_data_default_enabled_false() {
        let json = r#"{"enabled":false,"allowed_directories":[],"writable_directories":[],"blocked_commands":[],"max_read_bytes":0,"max_write_bytes":0,"shell_timeout_ms":0,"max_shell_output_bytes":0,"max_shell_output_lines":0}"#;
        let config: SandboxConfigData = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert!(config.allowed_directories.is_empty());
        assert!(config.undo_db_path.is_none());
    }
}
