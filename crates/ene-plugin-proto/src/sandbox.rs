use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

/// Serializable sandbox configuration data (POD).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", default)]
pub struct SandboxConfigData {
    /// Whether the sandbox is enabled.
    pub enabled: bool,
    /// Directories allowed for read access.
    pub allowed_directories: Vec<String>,
    /// Directories allowed for write access.
    pub writable_directories: Vec<String>,
    /// Regex patterns for blocked shell commands.
    pub blocked_commands: Vec<String>,
    /// Maximum bytes per read operation.
    pub max_read_bytes: usize,
    /// Maximum bytes per write operation.
    pub max_write_bytes: usize,
    /// Shell command timeout in milliseconds.
    pub shell_timeout_ms: u64,
    /// Maximum bytes in shell output.
    pub max_shell_output_bytes: usize,
    /// Maximum lines in shell output.
    pub max_shell_output_lines: usize,
    /// Path to the shared host-service socket. Plugin binaries open this
    /// socket and select a passenger service (currently `db`) with
    /// [`crate::HostServiceRequest::Open`].
    #[serde(default)]
    pub host_service_socket: Option<String>,
    /// Path to the DB socket. Kept for compatibility with plugins that
    /// only read this field; the host sets it to the same path as
    /// [`Self::host_service_socket`].
    pub db_socket: Option<String>,
    /// Pre-shared auth token for the host-service `db` passenger. The
    /// plugin binary must present this token in
    /// [`crate::HostServiceRequest::Open`] before any DB request. `None`
    /// disables DB access for this plugin.
    pub db_auth_token: Option<String>,
    /// Pre-shared auth token for the host-service `credential` passenger.
    /// The plugin presents it in [`crate::HostServiceRequest::Open`] for the
    /// credential service. Kept separate from `db_auth_token` so DB access
    /// never implies credential access. `None` disables credential access
    /// for this plugin (older plugins that predate the credential service
    /// never receive the field, so their handshake still succeeds).
    #[serde(default)]
    pub credential_auth_token: Option<String>,
}

impl Default for SandboxConfigData {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_directories: vec![".".to_string()],
            writable_directories: vec![".".to_string()],
            blocked_commands: default_blocked_commands(),
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            shell_timeout_ms: default_shell_timeout_ms(),
            max_shell_output_bytes: default_max_shell_output_bytes(),
            max_shell_output_lines: default_max_shell_output_lines(),
            host_service_socket: None,
            db_socket: None,
            db_auth_token: None,
            credential_auth_token: None,
        }
    }
}

impl SandboxConfigData {
    /// Enforce safe floors for tool sandbox settings.
    ///
    /// - Unions the default dangerous-command blocklist into `blocked_commands`
    ///   (user patterns are kept).
    /// - Replaces zero-valued size/timeout limits with code defaults.
    /// - When `enabled` is true and `allowed_directories` is empty, inserts
    ///   `["."]` so path checks have a usable allowlist.
    ///
    /// Does **not** force `enabled = true` or cap user-inflated limits.
    pub fn sanitize(&mut self) {
        for pattern in default_blocked_commands() {
            if !self.blocked_commands.iter().any(|p| p == &pattern) {
                self.blocked_commands.push(pattern);
            }
        }
        if self.enabled && self.allowed_directories.is_empty() {
            self.allowed_directories.push(".".to_string());
        }
        if self.max_read_bytes == 0 {
            self.max_read_bytes = default_max_read_bytes();
        }
        if self.max_write_bytes == 0 {
            self.max_write_bytes = default_max_write_bytes();
        }
        if self.shell_timeout_ms == 0 {
            self.shell_timeout_ms = default_shell_timeout_ms();
        }
        if self.max_shell_output_bytes == 0 {
            self.max_shell_output_bytes = default_max_shell_output_bytes();
        }
        if self.max_shell_output_lines == 0 {
            self.max_shell_output_lines = default_max_shell_output_lines();
        }
    }
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
            host_service_socket: Some("/tmp/ene-host-service.sock".into()),
            db_socket: Some("/tmp/ene-host-service.sock".into()),
            db_auth_token: Some("ene-db-deadbeef".into()),
            credential_auth_token: Some("ene-cred-deadbeef".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deser: SandboxConfigData = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deser);
    }

    #[test]
    fn sanitize_restores_safe_floors_from_zeros_and_empty_blocklist() {
        let json = r#"{"enabled":false,"allowed_directories":[],"writable_directories":[],"blocked_commands":[],"max_read_bytes":0,"max_write_bytes":0,"shell_timeout_ms":0,"max_shell_output_bytes":0,"max_shell_output_lines":0}"#;
        let mut config: SandboxConfigData = serde_json::from_str(json).unwrap();
        config.sanitize();
        assert!(!config.enabled);
        assert!(config.allowed_directories.is_empty());
        let defaults = default_blocked_commands();
        for pattern in &defaults {
            assert!(
                config.blocked_commands.contains(pattern),
                "missing default block pattern: {pattern}"
            );
        }
        assert_eq!(config.max_read_bytes, default_max_read_bytes());
        assert_eq!(config.max_write_bytes, default_max_write_bytes());
        assert_eq!(config.shell_timeout_ms, default_shell_timeout_ms());
        assert_eq!(
            config.max_shell_output_bytes,
            default_max_shell_output_bytes()
        );
        assert_eq!(
            config.max_shell_output_lines,
            default_max_shell_output_lines()
        );
    }

    #[test]
    fn sanitize_fills_allowed_directories_when_enabled_and_empty() {
        let mut config = SandboxConfigData {
            enabled: true,
            allowed_directories: vec![],
            ..SandboxConfigData::default()
        };
        config.sanitize();
        assert_eq!(config.allowed_directories, vec![".".to_string()]);
    }

    #[test]
    fn sanitize_keeps_user_patterns_and_non_zero_limits() {
        let mut config = SandboxConfigData {
            blocked_commands: vec!["custom_danger".into()],
            max_read_bytes: 1024,
            max_write_bytes: 2048,
            shell_timeout_ms: 5_000,
            max_shell_output_bytes: 4096,
            max_shell_output_lines: 100,
            ..SandboxConfigData::default()
        };
        config.sanitize();
        assert!(config.blocked_commands.iter().any(|p| p == "custom_danger"));
        for pattern in default_blocked_commands() {
            assert!(config.blocked_commands.contains(&pattern));
        }
        assert_eq!(config.max_read_bytes, 1024);
        assert_eq!(config.max_write_bytes, 2048);
        assert_eq!(config.shell_timeout_ms, 5_000);
        assert_eq!(config.max_shell_output_bytes, 4096);
        assert_eq!(config.max_shell_output_lines, 100);
    }
}
