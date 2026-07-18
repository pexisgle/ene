fn default_blocked_commands() -> Vec<String> {
    vec![
        r"rm\s+-rf\s+/".to_string(),
        r"dd\s+if=".to_string(),
        r"mkfs".to_string(),
        r"sudo\s+".to_string(),
        r":\s*\{\s*\|\s*&\s*;\s*\}".to_string(),
    ]
}

const fn default_max_read_bytes() -> usize {
    50 * 1024
}

const fn default_max_write_bytes() -> usize {
    1024 * 1024
}

const fn default_shell_timeout_ms() -> u64 {
    120_000
}

const fn default_max_shell_output_bytes() -> usize {
    50 * 1024
}

const fn default_max_shell_output_lines() -> usize {
    2000
}

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
        #[serde(skip_deserializing, default = "default_blocked_commands", skip_serializing)]
        #[schemars(skip)]
        pub blocked_commands: Vec<String> = default_blocked_commands(),
        /// Maximum bytes per read operation.
        #[serde(skip_deserializing, default = "default_max_read_bytes", skip_serializing)]
        #[schemars(skip)]
        pub max_read_bytes: usize = default_max_read_bytes(),
        /// Maximum bytes per write operation.
        #[serde(skip_deserializing, default = "default_max_write_bytes", skip_serializing)]
        #[schemars(skip)]
        pub max_write_bytes: usize = default_max_write_bytes(),
        /// Shell command timeout in milliseconds.
        #[serde(skip_deserializing, default = "default_shell_timeout_ms", skip_serializing)]
        #[schemars(skip)]
        pub shell_timeout_ms: u64 = default_shell_timeout_ms(),
        /// Maximum bytes in shell output.
        #[serde(skip_deserializing, default = "default_max_shell_output_bytes", skip_serializing)]
        #[schemars(skip)]
        pub max_shell_output_bytes: usize = default_max_shell_output_bytes(),
        /// Maximum lines in shell output.
        #[serde(skip_deserializing, default = "default_max_shell_output_lines", skip_serializing)]
        #[schemars(skip)]
        pub max_shell_output_lines: usize = default_max_shell_output_lines(),
        /// Path to the per-tool DB socket. Tool binaries connect to this
        /// Unix socket to access the core DB server for typed CRUD operations.
        pub db_socket: Option<String> = None,
        /// Pre-shared auth token for the per-tool DB IPC server. The
        /// tool binary must present this token in a [`ene_tool_db::DbRequest::Handshake`]
        /// before any other request. `None` disables DB access for
        /// this tool.
        pub db_auth_token: Option<String> = None,
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
            db_auth_token: Some("ene-db-deadbeef".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deser: SandboxConfigData = serde_json::from_str(&json).unwrap();
        assert_eq!(config.enabled, deser.enabled);
        assert_eq!(config.allowed_directories, deser.allowed_directories);
        assert_eq!(config.db_socket, deser.db_socket);
        assert_eq!(config.db_auth_token, deser.db_auth_token);
        // Hidden fields always use code defaults on deserialize.
        let defaults = SandboxConfigData::default();
        assert_eq!(deser.blocked_commands, defaults.blocked_commands);
        assert_eq!(deser.max_read_bytes, defaults.max_read_bytes);
        assert_eq!(deser.max_write_bytes, defaults.max_write_bytes);
        assert_eq!(deser.shell_timeout_ms, defaults.shell_timeout_ms);
        assert_eq!(
            deser.max_shell_output_bytes,
            defaults.max_shell_output_bytes
        );
        assert_eq!(
            deser.max_shell_output_lines,
            defaults.max_shell_output_lines
        );
    }

    #[test]
    fn sandbox_config_data_default_enabled_false() {
        let json = r#"{"enabled":false,"allowed_directories":[],"writable_directories":[],"blocked_commands":[],"max_read_bytes":0,"max_write_bytes":0,"shell_timeout_ms":0,"max_shell_output_bytes":0,"max_shell_output_lines":0}"#;
        let config: SandboxConfigData = serde_json::from_str(json).unwrap();
        let defaults = SandboxConfigData::default();
        assert!(!config.enabled);
        assert!(config.allowed_directories.is_empty());
        assert!(config.db_socket.is_none());
        assert_eq!(config.blocked_commands, defaults.blocked_commands);
        assert_eq!(config.max_read_bytes, defaults.max_read_bytes);
        assert_eq!(config.max_write_bytes, defaults.max_write_bytes);
        assert_eq!(config.shell_timeout_ms, defaults.shell_timeout_ms);
        assert_eq!(
            config.max_shell_output_bytes,
            defaults.max_shell_output_bytes
        );
        assert_eq!(
            config.max_shell_output_lines,
            defaults.max_shell_output_lines
        );
    }
}
