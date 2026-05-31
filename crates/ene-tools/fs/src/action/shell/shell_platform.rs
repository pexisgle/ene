use std::time::Duration;
use tokio::process::Command;

pub async fn execute_shell_command(
    command: &str,
    cwd: &str,
    timeout_duration: Duration,
) -> Result<std::process::Output, std::io::Error> {
    #[cfg(unix)]
    let fut = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output();

    #[cfg(windows)]
    let fut = Command::new("cmd")
        .arg("/C")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output();

    match tokio::time::timeout(timeout_duration, fut).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Command timed out",
        )),
    }
}
