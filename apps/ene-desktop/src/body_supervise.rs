//! Optional Body child. Chat and settings do not wait for this process.
//!
//! Projection IPC matches `apps/ene-body/README.md`: length-prefixed
//! MessagePack on `--ipc-stdio`. Commands are [`ene_body::ParentToBody`]
//! only — secrets, chat text, and Task commands have no variant.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use ene_body::ipc::{BodyToParent, ParentToBody, decode_body, encode_parent};

/// Health of the overlay child as observed by desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStatus {
    Absent,
    Spawned,
    Exited,
}

/// Supervises at most one `ene-body` child by executable path.
#[derive(Default)]
pub struct BodySupervisor {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    events: Option<Receiver<BodyToParent>>,
    exe: Option<PathBuf>,
    last_event: Option<String>,
}

impl Drop for BodySupervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl BodySupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves `ene-body` from `ENE_BODY_PATH`, `CARGO_TARGET_DIR`, or a
    /// sibling of this process. Missing official VRM is still [`BodyStatus::Absent`].
    #[must_use]
    pub fn locate_binary() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("ENE_BODY_PATH") {
            let candidate = PathBuf::from(path);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
            for profile in ["debug", "release"] {
                let candidate = PathBuf::from(&dir).join(profile).join(body_binary_name());
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let Ok(exe) = std::env::current_exe() else {
            return None;
        };
        let sibling = exe.parent()?.join(body_binary_name());
        if sibling.is_file() {
            return Some(sibling);
        }
        let debug = exe.parent()?.parent()?.join(body_binary_name());
        debug.is_file().then_some(debug)
    }

    /// Spawns `exe --ipc-stdio` if the path exists. Does not wait for Ready.
    /// Missing official VRM / missing binary is [`BodyStatus::Absent`], not a
    /// fake overlay.
    pub fn spawn_if_present(&mut self, exe: &Path) -> BodyStatus {
        if !exe.is_file() {
            self.exe = Some(exe.to_path_buf());
            return BodyStatus::Absent;
        }
        self.shutdown();
        match Command::new(exe)
            .arg("--ipc-stdio")
            .env("ENE_BODY_SKIP_GPU", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                let stdout = child.stdout.take();
                let (tx, rx) = mpsc::channel();
                let reader = stdout.map(|mut stdout| {
                    thread::spawn(move || {
                        let mut buf = Vec::new();
                        let mut chunk = [0_u8; 4096];
                        loop {
                            match stdout.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(n) => {
                                    buf.extend_from_slice(&chunk[..n]);
                                    loop {
                                        match decode_body(&buf) {
                                            Ok((message, used)) => {
                                                buf.drain(..used);
                                                if tx.send(message).is_err() {
                                                    return;
                                                }
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    })
                });
                self.child = Some(child);
                self.stdin = stdin;
                self.reader = reader;
                self.events = Some(rx);
                self.exe = Some(exe.to_path_buf());
                self.last_event = None;
                BodyStatus::Spawned
            }
            Err(_) => BodyStatus::Absent,
        }
    }

    /// Sends one projection command. Chat is not blocked on Ready.
    ///
    /// # Errors
    ///
    /// Missing child or encode/write failure. Errors never include frame bytes.
    pub fn send_projection(&mut self, message: &ParentToBody) -> Result<(), BodySuperviseError> {
        let stdin = self.stdin.as_mut().ok_or(BodySuperviseError::NotRunning)?;
        let bytes = encode_parent(message).map_err(|_| BodySuperviseError::Encode)?;
        stdin
            .write_all(&bytes)
            .map_err(|_| BodySuperviseError::Write)?;
        stdin.flush().map_err(|_| BodySuperviseError::Write)
    }

    pub fn poll(&mut self) -> BodyStatus {
        self.drain_events();
        match self.child.as_mut() {
            None => BodyStatus::Absent,
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    self.drop_io();
                    self.child = None;
                    BodyStatus::Exited
                }
                Ok(None) => BodyStatus::Spawned,
                Err(_) => {
                    self.drop_io();
                    self.child = None;
                    BodyStatus::Exited
                }
            },
        }
    }

    /// Kind of the last Body→parent event, for tests. Never a secret or chat body.
    #[must_use]
    pub fn last_event_kind(&self) -> Option<&str> {
        self.last_event.as_deref()
    }

    pub fn shutdown(&mut self) {
        if self.stdin.is_some() {
            match self.send_projection(&ParentToBody::Shutdown) {
                Ok(()) | Err(_) => {}
            }
        }
        self.drop_io();
        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(()) | Err(_) => {}
            }
            match child.wait() {
                Ok(_) | Err(_) => {}
            }
        }
        if let Some(reader) = self.reader.take() {
            match reader.join() {
                Ok(()) | Err(_) => {}
            }
        }
    }

    fn drain_events(&mut self) {
        let mut disconnected = false;
        let mut last = None;
        if let Some(events) = self.events.as_mut() {
            loop {
                match events.try_recv() {
                    Ok(message) => last = Some(event_kind(&message).to_string()),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(kind) = last {
            self.last_event = Some(kind);
        }
        if disconnected {
            self.events = None;
        }
    }

    fn drop_io(&mut self) {
        self.stdin = None;
        self.events = None;
    }
}

fn event_kind(message: &BodyToParent) -> &'static str {
    match message {
        BodyToParent::Ready(_) => "Ready",
        BodyToParent::GpuFail(_) => "GpuFail",
        BodyToParent::AssetFail(_) => "AssetFail",
        BodyToParent::HealthTick(_) => "HealthTick",
        BodyToParent::LocalUi(_) => "LocalUi",
        BodyToParent::CleanExit => "CleanExit",
    }
}

fn body_binary_name() -> &'static str {
    if cfg!(windows) {
        "ene-body.exe"
    } else {
        "ene-body"
    }
}

/// Projection IPC failures. Reasons never include frame bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BodySuperviseError {
    #[error("body is not running")]
    NotRunning,
    #[error("projection encode failed")]
    Encode,
    #[error("projection write failed")]
    Write,
}

#[cfg(test)]
mod tests {
    use super::{BodyStatus, BodySupervisor};
    use ene_body::ParentToBody;
    use std::path::Path;

    #[test]
    fn missing_binary_is_absent_not_a_character() {
        let mut supervisor = BodySupervisor::new();
        let status = supervisor.spawn_if_present(Path::new("/no/such/ene-body"));
        assert_eq!(status, BodyStatus::Absent);
        assert_eq!(supervisor.poll(), BodyStatus::Absent);
    }

    #[test]
    fn projection_commands_have_no_secret_or_chat_variant() {
        let encoded = ene_body::ipc::encode_parent(&ParentToBody::Show).expect("encode");
        let blob = String::from_utf8_lossy(&encoded);
        assert!(
            !blob.contains("sk-") && !blob.contains("chat") && !blob.contains("task"),
            "projection must not carry domain text"
        );
    }
}
