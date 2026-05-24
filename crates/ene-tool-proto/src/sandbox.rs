// SandboxConfig のシリアライズ可能データ型（POD）
//
// バリデーションロジックは含まず、ene-tools/fs::Sandbox で構築・検証する。
// core → host 間のIPC通信で使用される。
ene_config::define_config!(
    "sandbox",
    #[derive(PartialEq, Eq)]
    pub struct SandboxConfigData {
        pub enabled: bool = true,
        pub allowed_directories: Vec<String> = vec![".".to_string()],
        pub writable_directories: Vec<String> = vec![".".to_string()],
        pub blocked_commands: Vec<String> = vec![
            r"rm\s+-rf\s+/".to_string(),
            r"dd\s+if=".to_string(),
            r"mkfs".to_string(),
            r"sudo\s+".to_string(),
            r":\s*\{\s*\|\s*&\s*;\s*\}".to_string(),
        ],
        pub max_read_bytes: usize = 50 * 1024,
        pub max_write_bytes: usize = 1024 * 1024,
        pub shell_timeout_ms: u64 = 120_000,
        pub max_shell_output_bytes: usize = 50 * 1024,
        pub max_shell_output_lines: usize = 2000,
        pub undo_db_path: Option<String> = None,
    }
);

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
