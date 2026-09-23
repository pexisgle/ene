use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct DetachedHost {
    pub pid: u32,
}

impl Drop for DetachedHost {
    fn drop(&mut self) {
        // Host outlives the GUI. Do not send a signal here.
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("ene-core binary was not found")]
    MissingBinary,
    #[error("failed to detach ene-core serve: {0}")]
    Spawn(String),
}

#[must_use]
pub fn host_is_serving(data_dir: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(ene_client::socket_path(data_dir)).is_ok()
    }
    #[cfg(windows)]
    {
        probe_windows_client_pipe(data_dir)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _data_dir = data_dir;
        false
    }
}

#[cfg(windows)]
fn probe_windows_client_pipe(data_dir: &Path) -> bool {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    let pipe = format!(r"\\.\pipe\ene-{tag:016x}");
    std::fs::metadata(pipe).is_ok()
}

#[must_use]
pub fn locate_host_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("ENE_CORE_PATH") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return None;
    };
    let sibling = exe.parent()?.join(host_binary_name());
    sibling.is_file().then_some(sibling)
}

fn host_binary_name() -> &'static str {
    if cfg!(windows) {
        "ene-core.exe"
    } else {
        "ene-core"
    }
}

pub fn detach_serve(data_dir: &Path, host_bin: &Path) -> Result<DetachedHost, LaunchError> {
    if !host_bin.is_file() {
        return Err(LaunchError::MissingBinary);
    }
    let mut command = Command::new(host_bin);
    command
        .arg("serve")
        .env("ENE_DATA_DIR", data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    let mut child = command
        .spawn()
        .map_err(|error| LaunchError::Spawn(error.kind().to_string()))?;
    let pid = child.id();
    std::thread::spawn(move || match child.wait() {
        Ok(_) | Err(_) => {}
    });
    Ok(DetachedHost { pid })
}

#[cfg(all(test, unix))]
mod tests {
    use super::detach_serve;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    #[test]
    fn detach_does_not_kill_child_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = dir.path().join("ene-core");
        let pid_file = dir.path().join("stub.pid");
        let script = format!(
            "#!/bin/sh\ntrap '' HUP\necho $$ > {}\nexec sleep 30\n",
            pid_file.display()
        );
        fs::write(&stub, script).expect("write stub");
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
        let detached = detach_serve(dir.path(), &stub).expect("detach");
        let started = Instant::now();
        let pid = loop {
            if let Ok(text) = fs::read_to_string(&pid_file)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "stub must write pid"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(detached.pid, pid);
        drop(detached);
        std::thread::sleep(Duration::from_millis(100));
        let still = std::path::Path::new("/proc").join(pid.to_string()).exists();
        match std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
        {
            Ok(_) | Err(_) => {}
        }
        assert!(still, "Host stub must survive DetachedHost drop");
    }
}
