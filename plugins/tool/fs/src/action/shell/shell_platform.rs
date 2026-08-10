//! Subshell argv construction for the host-mediated `process` broker.

#[cfg(unix)]
pub fn shell_argv(command: &str) -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), command.to_string()]
}

#[cfg(windows)]
pub fn shell_argv(command: &str) -> Vec<String> {
    vec!["cmd".to_string(), "/C".to_string(), command.to_string()]
}
