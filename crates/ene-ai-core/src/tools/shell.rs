use std::path::Path;
use std::time::Duration;
use crate::sandbox::SandboxConfig;
use crate::error::AiCoreError;
use crate::tools::truncate::Truncate;
use super::definition::ToolDefinition;

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
    }
}

/// シェルコマンドを実行
pub async fn shell_exec(
    command: &str,
    description: &str,
    timeout: Option<u64>,
    workdir: Option<&str>,
    sandbox: &SandboxConfig,
) -> Result<String, AiCoreError> {
    // ブロックコマンドチェック
    sandbox.is_command_blocked(command)?;

    let cwd = if let Some(wd) = workdir {
        sandbox.resolve_and_check(Path::new(wd), true)?.to_string_lossy().to_string()
    } else {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    };

    let timeout_ms = timeout.unwrap_or(sandbox.shell_timeout_ms);
    let timeout_duration = Duration::from_millis(timeout_ms);

    #[cfg(unix)]
    let output = {
        tokio::time::timeout(
            timeout_duration,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&cwd)
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await
    };

    #[cfg(windows)]
    let output = {
        tokio::time::timeout(
            timeout_duration,
            tokio::process::Command::new("cmd")
                .arg("/C")
                .arg(command)
                .current_dir(&cwd)
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await
    };

    let result = match output {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(AiCoreError::ToolExecutionError(format!("Failed to execute command: {e}"))),
        Err(_) => return Err(AiCoreError::ShellTimeout(timeout_ms)),
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

    // 出力圧縮
    let truncated = Truncate::tail(
        &full_output,
        sandbox.max_shell_output_lines,
        sandbox.max_shell_output_bytes,
    );

    let mut output_text = truncated.content;
    if result.status.code() != Some(0) {
        output_text = format!("[Exit code: {}]\n{}", result.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".to_string()), output_text);
    }

    if truncated.truncated {
        output_text.push_str(&format!("\n\n(Shell output was truncated. Full output: {} bytes, {} lines)", full_output.len(), full_output.lines().count()));
    }

    // Description を先頭に追加
    let final_output = format!("# {}\n{}", description, output_text);

    Ok(final_output)
}
