use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CtlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("spawn: {0}")]
    Spawn(String),
    #[error("ready: {0}")]
    Ready(String),
    #[error("codec: {0}")]
    Codec(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already running")]
    AlreadyRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiReady {
    pub bind: String,
    pub url: String,
    #[serde(default)]
    pub token_file: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

pub const PID_FILE: &str = "ene-core.pid";

const CORE_BIN: &str = if cfg!(windows) {
    "ene-core.exe"
} else {
    "ene-core"
};

#[must_use]
pub fn pid_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PID_FILE)
}

#[must_use]
pub fn binary_in_dir(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(CORE_BIN);
    candidate.is_file().then_some(candidate)
}

pub fn find_ene_core_binary() -> Result<PathBuf, CtlError> {
    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop();
        if exe.ends_with("deps") {
            exe.pop();
        }
        if let Some(path) = binary_in_dir(&exe) {
            return Ok(path);
        }
    }

    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if let Some(path) = binary_in_dir(&dir) {
            return Ok(path);
        }
    }

    Err(CtlError::NotFound(format!(
        "{CORE_BIN} not found next to current executable or on PATH"
    )))
}

pub fn parse_api_json(text: &str) -> Result<ApiReady, CtlError> {
    serde_json::from_str(text).map_err(|err| CtlError::Codec(err.to_string()))
}

pub fn read_api_ready(path: &Path) -> Result<ApiReady, CtlError> {
    let text = std::fs::read_to_string(path)?;
    let mut ready = parse_api_json(&text)?;
    if ready.url.is_empty() {
        return Err(CtlError::Ready("api.json missing url".to_owned()));
    }
    if ready.token.is_none() {
        let token_path = path
            .parent()
            .ok_or_else(|| CtlError::Ready("api.json has no parent dir".to_owned()))?
            .join(ready.token_file.as_deref().unwrap_or("api.token"));
        if token_path.is_file() {
            let token = std::fs::read_to_string(&token_path)?;
            let token = token.trim();
            if !token.is_empty() {
                ready.token = Some(token.to_owned());
            }
        }
    }
    Ok(ready)
}

pub async fn wait_for_api_json(path: &Path, timeout: Duration) -> Result<ApiReady, CtlError> {
    let deadline = Instant::now() + timeout;
    loop {
        if path.is_file()
            && let Ok(ready) = read_api_ready(path)
        {
            return Ok(ready);
        }
        if Instant::now() >= deadline {
            return Err(CtlError::Ready(format!(
                "timed out waiting for {}",
                path.display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(unix))]
fn process_alive(pid: u32) -> bool {
    let _ = pid;
    false
}

pub async fn start_core(data_dir: &Path, foreground: bool) -> Result<(), CtlError> {
    std::fs::create_dir_all(data_dir)?;
    let pid_path = pid_file_path(data_dir);
    if pid_path.is_file()
        && let Ok(text) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = text.trim().parse::<u32>()
        && process_alive(pid)
    {
        return Err(CtlError::AlreadyRunning);
    }

    let bin = find_ene_core_binary()?;
    let api_json = data_dir.join("api.json");
    if api_json.is_file() {
        std::fs::remove_file(&api_json).map_err(CtlError::Io)?;
    }
    let token_path = data_dir.join("api.token");

    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("--data-dir").arg(data_dir);
    if !foreground {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| CtlError::Spawn(err.to_string()))?;
    let pid = child.id();

    let ready = wait_for_api_json(&api_json, Duration::from_mins(1)).await?;

    if !foreground {
        if child.try_wait()?.is_some() {
            return Err(CtlError::Spawn(
                "ene-core exited before becoming ready".to_owned(),
            ));
        }
        std::fs::write(&pid_path, format!("{pid}\n"))?;
    }

    println!("{}", ready.url);
    if foreground {
        let status = child.wait()?;
        if !status.success() {
            return Err(CtlError::Spawn(format!("ene-core exited with {status}")));
        }
    } else {
        println!("pid {pid} ({})", pid_path.display());
        println!("token file: {}", token_path.display());
        println!(
            "Connect: ENE_API_URL={} ENE_API_TOKEN=<read {}> ene-ctl status",
            ready.url,
            token_path.display()
        );
    }
    Ok(())
}

pub fn stop_core(data_dir: &Path) -> Result<(), CtlError> {
    let pid_path = pid_file_path(data_dir);
    if !pid_path.is_file() {
        return Err(CtlError::NotFound(format!(
            "no pid file at {}",
            pid_path.display()
        )));
    }
    let pid_text = std::fs::read_to_string(&pid_path)?;
    let pid: u32 = pid_text
        .trim()
        .parse()
        .map_err(|_| CtlError::Codec(format!("invalid pid in {}", pid_path.display())))?;
    kill_pid(pid)?;
    if let Err(err) = std::fs::remove_file(&pid_path) {
        eprintln!("warning: could not remove {}: {err}", pid_path.display());
    }
    println!("stopped ene-core (pid {pid})");
    Ok(())
}

#[cfg(unix)]
fn kill_pid(pid: u32) -> Result<(), CtlError> {
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()?;
    if !status.success() {
        return Err(CtlError::Spawn(format!("kill {pid} failed with {status}")));
    }
    Ok(())
}

#[cfg(not(unix))]
fn kill_pid(pid: u32) -> Result<(), CtlError> {
    let _ = pid;
    Err(CtlError::NotFound(
        "ene-ctl core stop is only supported on Unix".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "unit tests")]

    use super::*;
    use std::io::Write;

    #[test]
    fn parse_api_json_reads_url_and_token_file() {
        let text =
            r#"{"bind":"127.0.0.1:8080","url":"http://127.0.0.1:8080","token_file":"api.token"}"#;
        let ready = parse_api_json(text).unwrap();
        assert_eq!(ready.url, "http://127.0.0.1:8080");
        assert_eq!(ready.token_file.as_deref(), Some("api.token"));
    }

    #[test]
    fn read_api_ready_loads_token_from_sibling_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api.json");
        std::fs::write(
            &path,
            r#"{"bind":"x","url":"http://x","token_file":"api.token"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("api.token"), "secret\n").unwrap();
        let ready = read_api_ready(&path).unwrap();
        assert_eq!(ready.token.as_deref(), Some("secret"));
    }

    #[test]
    fn read_api_ready_rejects_empty_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api.json");
        std::fs::write(&path, r#"{"bind":"x","url":""}"#).unwrap();
        let err = read_api_ready(&path).unwrap_err();
        assert!(matches!(err, CtlError::Ready(_)));
    }

    #[test]
    fn binary_in_dir_finds_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(CORE_BIN);
        let mut file = std::fs::File::create(&bin).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        assert_eq!(binary_in_dir(dir.path()), Some(bin));
    }

    #[test]
    fn pid_file_path_is_under_data_dir() {
        let dir = Path::new("/tmp/ene-data");
        assert_eq!(pid_file_path(dir), Path::new("/tmp/ene-data/ene-core.pid"));
    }
}
