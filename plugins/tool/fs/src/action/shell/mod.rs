mod shell_platform;

use self::shell_platform::execute_shell_command;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_tool_sdk::prelude::*;
use ene_util::truncate::Truncate;
use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

pub async fn shell_exec(
    command: &str,
    description: &str,
    timeout: Option<u64>,
    workdir: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    sandbox.is_command_blocked(command)?;

    let cwd = if let Some(wd) = workdir {
        sandbox
            .resolve_and_check(Path::new(wd), true)?
            .to_string_lossy()
            .to_string()
    } else {
        std::env::current_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
    };

    let timeout_ms = timeout.unwrap_or(sandbox.shell_timeout_ms);
    let timeout_duration = Duration::from_millis(timeout_ms);

    let result = execute_shell_command(command, &cwd, timeout_duration).await;

    let result = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(ToolError::timeout(format!(
                "Command timed out after {timeout_ms} ms"
            )));
        }
        Err(e) => {
            return Err(ToolError::execution_failed(format!(
                "Failed to execute command: {e}"
            )));
        }
    };

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    let mut full_output = stdout.to_string();
    if !stderr.is_empty() {
        if !full_output.is_empty() {
            full_output.push('\n');
        }
        full_output.push_str(&stderr);
    }

    if full_output.is_empty() {
        full_output = "(no output)".to_string();
    }

    let truncated = Truncate::tail(
        &full_output,
        sandbox.max_shell_output_lines,
        sandbox.max_shell_output_bytes,
    );

    let mut output_text = truncated.content;
    if result.status.code() != Some(0) {
        output_text = format!(
            "[Exit code: {}]\n{}",
            result
                .status
                .code()
                .map_or_else(|| "?".to_string(), |c| c.to_string()),
            output_text
        );
    }

    if truncated.truncated {
        // `fmt::Error` is `Copy`, so `drop()` would itself trip
        // `clippy::dropping_copy_types`; writing into a `String` via
        // `fmt::Write` never actually fails.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "fmt::Write to a String is infallible in practice"
        )]
        let _ = write!(
            output_text,
            "\n\n(Shell output was truncated. Full output: {} bytes, {} lines)",
            full_output.len(),
            full_output.lines().count()
        );
    }

    let final_output = format!("# {description}\n{output_text}");

    Ok(final_output)
}

#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "shell",
    name = "execute",
    summary = "Executes a shell command with optional timeout and security measures.",
    description = "Executes a given shell command with optional timeout, ensuring proper handling and security measures. All commands run in the current working directory by default. Use the workdir parameter if you need to run a command in a different directory. AVOID using 'cd <directory> && <command>' patterns - use workdir instead. Clear, concise description of what this command does in 5-10 words is required.",
    category = "Shell",
    keywords_primary = "shell, command, execute, terminal, bash",
    side_effects = "System { privileged: true }"
)]
pub struct ShellAction {
    /// The command to execute.
    command: String,
    /// Clear, concise description of what this command does in 5-10 words.
    #[serde(default)]
    description: Option<String>,
    /// Optional timeout in milliseconds.
    #[serde(default)]
    timeout: Option<u64>,
    /// The working directory to run the command in. Defaults to current directory.
    #[serde(default)]
    workdir: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl ShellAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            command: String::new(),
            description: None,
            timeout: None,
            workdir: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);

        let description = self.description.as_deref().unwrap_or("");
        let command = self.command.as_str();

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::ShellCommand,
            command,
            description,
        )?;

        shell_exec(
            command,
            description,
            self.timeout,
            self.workdir.as_deref(),
            sandbox.config(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_sdk::ToolAction;

    #[test]
    fn schema_does_not_require_description() {
        let action = ShellAction::new(crate::utils::default_sandbox());
        let def = action.definition();
        let required = def
            .parameters
            .get("required")
            .and_then(|r| r.as_array())
            .expect("schema must declare a `required` array");
        let required: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(required, vec!["command"]);
        assert!(
            def.parameters
                .get("properties")
                .and_then(|p| p.get("description"))
                .is_some(),
            "schema should still document the optional `description` field"
        );
    }
}
