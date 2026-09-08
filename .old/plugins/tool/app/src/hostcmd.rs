use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) const HOST_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn stdout_text(bin: &str, args: &[&str]) -> Result<String, String> {
    let bytes = stdout_bytes_timeout(bin, args, HOST_COMMAND_TIMEOUT)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn stdout_bytes_timeout(
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin} has no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let waited = wait_child(&mut child, bin, timeout);
    let buf = reader
        .join()
        .map_err(|_| format!("{bin} stdout reader panicked"))?
        .map_err(|err| err.to_string())?;
    waited?;
    Ok(buf)
}

pub(crate) fn pipe_bytes(
    bin: &str,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).map_err(|err| err.to_string())?;
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin} has no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let waited = wait_child(&mut child, bin, timeout);
    let buf = reader
        .join()
        .map_err(|_| format!("{bin} stdout reader panicked"))?
        .map_err(|err| err.to_string())?;
    waited?;
    Ok(buf)
}

pub(crate) fn stdin_text(bin: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    wait_child(&mut child, bin, HOST_COMMAND_TIMEOUT)
}

pub(crate) fn run(bin: &str, args: &[&str]) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    wait_child(&mut child, bin, HOST_COMMAND_TIMEOUT)
}

pub(crate) fn wait_child(
    child: &mut std::process::Child,
    bin: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(format!("{bin} failed")),
            Ok(None) if Instant::now() >= deadline => {
                drop(child.kill());
                drop(child.wait());
                return Err(format!("{bin} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(err.to_string()),
        }
    }
}
