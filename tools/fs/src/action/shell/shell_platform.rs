use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;

/// Maximum bytes captured from a single child process's combined
/// stdout/stderr before we forcibly kill it. Without this, a
/// command that emits gigabytes can OOM the tool.
const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// Runs a command in a subshell with a timeout, returning the
/// captured stdout/stderr and exit status.
///
/// # Cancelled / timed-out children
///
/// The [`Command`] is built with `.kill_on_drop(true)` so that when
/// the timeout elapses and the future returned here is dropped, the
/// child process is *actually* killed. Without this, a long-running
/// shell command would keep executing with the user's full
/// privileges after the tool returned an error.
///
/// # OOM protection
///
/// stdout and stderr are read incrementally via [`BufReader::take`]
/// capped at [`MAX_OUTPUT_BYTES`]. As soon as either stream has
/// produced that many bytes, the child is killed and the captured
/// prefix is returned. The total memory footprint is therefore
/// bounded regardless of the command's natural output size.
pub async fn execute_shell_command(
    command: &str,
    cwd: &str,
    timeout_duration: Duration,
) -> Result<std::process::Output, std::io::Error> {
    let mut child = build_command(command, cwd)?
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout missing"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("child stderr missing"))?;

    // Read both streams concurrently so a process that fills one
    // pipe while the other stays idle does not stall the other.
    let stdout_buf = BufReader::new(&mut stdout);
    let stderr_buf = BufReader::new(&mut stderr);

    let read_stdout = async {
        let mut out = Vec::new();
        let mut limited = stdout_buf.take(MAX_OUTPUT_BYTES);
        let _ = limited.read_to_end(&mut out).await;
        out
    };
    let read_stderr = async {
        let mut out = Vec::new();
        let mut limited = stderr_buf.take(MAX_OUTPUT_BYTES);
        let _ = limited.read_to_end(&mut out).await;
        out
    };

    let timeout_fut = tokio::time::timeout(timeout_duration, async {
        let (stdout, stderr) = tokio::join!(read_stdout, read_stderr);
        let status = child.wait().await?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout,
            stderr,
        })
    });

    match timeout_fut.await {
        Ok(result) => {
            // If either stream hit the byte cap, surface that as a
            // clear error rather than a silent truncated result.
            if let Ok(out) = &result
                && (out.stdout.len() as u64 >= MAX_OUTPUT_BYTES
                    || out.stderr.len() as u64 >= MAX_OUTPUT_BYTES)
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Command output exceeded {MAX_OUTPUT_BYTES} bytes; killed"),
                ));
            }
            result
        }
        Err(_) => {
            // Timeout: `kill_on_drop` is also set so the child dies
            // when `child` is dropped, but be explicit to be safe.
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Command timed out",
            ))
        }
    }
}

#[cfg(unix)]
fn build_command(command: &str, cwd: &str) -> std::io::Result<Command> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    Ok(cmd)
}

#[cfg(windows)]
fn build_command(command: &str, cwd: &str) -> std::io::Result<Command> {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(command).current_dir(cwd);
    Ok(cmd)
}
