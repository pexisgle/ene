mod platform;

use super::definition::ToolDefinition;
use super::utility::truncate::Truncate;
use crate::error::AiCoreError;
use crate::sandbox::SandboxConfig;
use std::path::Path;
use std::time::Duration;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "shell".to_string(),
        description: concat!(
            "Executes a given shell command with optional timeout, ensuring proper handling and security measures. ",
            "All commands run in the current working directory by default. Use the workdir parameter if you need to run a command in a different directory. ",
            "AVOID using 'cd <directory> && <command>' patterns - use workdir instead. ",
            "Clear, concise description of what this command does in 5-10 words is required."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to execute" },
                "description": { "type": "string", "description": "Clear, concise description of what this command does in 5-10 words" },
                "timeout": { "type": "integer", "description": "Optional timeout in milliseconds" },
                "workdir": { "type": "string", "description": "The working directory to run the command in. Defaults to current directory." }
            },
            "required": ["command", "description"]
        }),
        category: Some(super::ToolCategory::Shell),
        keywords: vec!["shell".to_string(), "command".to_string(), "execute".to_string(), "terminal".to_string(), "bash".to_string()],
    }
}

pub async fn shell_exec(
    command: &str,
    description: &str,
    timeout: Option<u64>,
    workdir: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, AiCoreError> {
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

    let result = platform::execute_shell_command(command, &cwd, timeout_duration).await;

    let result = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(AiCoreError::ShellTimeout(timeout_ms));
        }
        Err(e) => {
            return Err(AiCoreError::ToolExecutionError(format!(
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
