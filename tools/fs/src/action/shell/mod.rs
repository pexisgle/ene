mod shell_platform;

use self::shell_platform::execute_shell_command;

use crate::utils::sandbox::SandboxConfig;
use ene_tool_common::truncate::Truncate;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use std::path::Path;
use std::time::Duration;

pub fn tool_definition() -> ToolSpec {
    let description = concat!(
        "Executes a given shell command with optional timeout, ensuring proper handling and security measures. ",
        "All commands run in the current working directory by default. Use the workdir parameter if you need to run a command in a different directory. ",
        "AVOID using 'cd <directory> && <command>' patterns - use workdir instead. ",
        "Clear, concise description of what this command does in 5-10 words is required."
    );
    ToolSpec {
        name: ToolName::new("shell.execute"),
        version: ToolVersion::default(),
        display_name: "Execute Shell Command".to_string(),
        summary: "Executes a shell command with optional timeout and security measures.".to_string(),
        description: description.to_string(),
        category: ToolCategory::Shell,
        keywords: KeywordSet::primary_only(["shell", "command", "execute", "terminal", "bash"]),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to execute" },
                "description": { "type": "string", "description": "Clear, concise description of what this command does in 5-10 words" },
                "timeout": { "type": "integer", "description": "Optional timeout in milliseconds" },
                "workdir": { "type": "string", "description": "The working directory to run the command in. Defaults to current directory." }
            },
            "required": ["command"]
        }),
        examples: vec![
            ToolExample {
                description: "List files in a directory".to_string(),
                input: serde_json::json!({"command": "ls -la", "description": "List files in directory", "workdir": "/tmp"}),
                output: Some("# List files in directory\ntotal 8\ndrwxrwxr-x 2 user user 4096 Jun  1 10:00 .\ndrwxrwxr-x 3 user user 4096 Jun  1 09:59 ..".to_string()),
            },
        ],
        caveats: Vec::new(),
        side_effects: SideEffects::default(),
        preconditions: Vec::new(),
        related: Vec::new(),
    }
}

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
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    };

    let timeout_ms = timeout.unwrap_or(sandbox.shell_timeout_ms);
    let timeout_duration = Duration::from_millis(timeout_ms);

    let result = execute_shell_command(command, &cwd, timeout_duration).await;

    let result = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(ToolError::Timeout {
                message: format!("Command timed out after {} ms", timeout_ms),
            });
        }
        Err(e) => {
            return Err(ToolError::ExecutionFailed {
                message: format!("Failed to execute command: {e}"),
            });
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
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string()),
            output_text
        );
    }

    if truncated.truncated {
        output_text.push_str(&format!(
            "\n\n(Shell output was truncated. Full output: {} bytes, {} lines)",
            full_output.len(),
            full_output.lines().count()
        ));
    }

    let final_output = format!("# {}\n{}", description, output_text);

    Ok(final_output)
}

/// Filesystem tool action to execute shell commands.
pub struct ShellAction {
    sandbox:
        std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<crate::utils::sandbox::Sandbox>>>>,
}

impl ShellAction {
    /// Creates a new `ShellAction` with the shared sandbox reference.
    pub fn new(
        sandbox: std::sync::Arc<
            std::sync::RwLock<Option<std::sync::Arc<crate::utils::sandbox::Sandbox>>>,
        >,
    ) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ene_tool_common::ToolAction for ShellAction {
    fn tool_name(&self) -> &'static str {
        "shell"
    }

    fn definition(&self) -> ToolSpec {
        tool_definition()
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let sandbox = {
            let guard = self.sandbox.read().unwrap_or_else(|e| e.into_inner());
            guard.clone().unwrap_or_else(|| {
                std::sync::Arc::new(crate::utils::sandbox::Sandbox::new(Default::default()))
            })
        };

        let args: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Failed to parse shell arguments: {e}"),
            })?;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments {
                message: "Missing 'command' field".to_string(),
            })?;
        let description = args["description"].as_str().unwrap_or("");
        let timeout = args["timeout"].as_u64();
        let workdir = args["workdir"].as_str();

        sandbox.check_permission(
            crate::utils::permission::DestructiveAction::ShellCommand,
            command,
            description,
        )?;

        shell_exec(command, description, timeout, workdir, sandbox.config()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_common::ToolAction;

    #[test]
    fn schema_does_not_require_description() {
        // The implementation defaults `description` to `""` when missing
        // (see L182), so the schema must not list it as required. Otherwise
        // a correctly-running call without a description gets rejected at
        // the JSON-schema validation step before reaching the body.
        let action = ShellAction::new(std::sync::Arc::new(std::sync::RwLock::new(None)));
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
