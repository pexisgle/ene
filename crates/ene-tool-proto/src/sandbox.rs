ene_config::define_tool_config!(
    "fs",
    #[derive(PartialEq, Eq)]
    /// Serializable sandbox configuration data (POD).
    pub struct SandboxConfigData {
        /// Whether the sandbox is enabled.
        pub enabled: bool = true,
        /// Directories allowed for read access.
        pub allowed_directories: Vec<String> = vec![".".to_string()],
        /// Directories allowed for write access.
        pub writable_directories: Vec<String> = vec![".".to_string()],
        /// Regex patterns for blocked shell commands.
        pub blocked_commands: Vec<String> = vec![
            r"rm\s+-rf\s+/".to_string(),
            r"dd\s+if=".to_string(),
            r"mkfs".to_string(),
            r"sudo\s+".to_string(),
            r":\s*\{\s*\|\s*&\s*;\s*\}".to_string(),
        ],
        /// Maximum bytes per read operation.
        pub max_read_bytes: usize = 50 * 1024,
        /// Maximum bytes per write operation.
        pub max_write_bytes: usize = 1024 * 1024,
        /// Shell command timeout in milliseconds.
        pub shell_timeout_ms: u64 = 120_000,
        /// Maximum bytes in shell output.
        pub max_shell_output_bytes: usize = 50 * 1024,
        /// Maximum lines in shell output.
        pub max_shell_output_lines: usize = 2000,
        /// Path to the per-tool DB socket. Tool binaries connect to this
        /// Unix socket to access the core DB server for typed CRUD operations.
        pub db_socket: Option<String> = None,
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
            db_socket: Some("/tmp/db.sock".into()),
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
        assert!(config.db_socket.is_none());
    }
}
