//! Optional Body child. Chat and settings do not wait for this process.
//!
//! Projection IPC is a later D integration. This module only records a spawn
//! path and whether a child is alive. Host is never told that Body crash is
//! Companion stop.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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
    exe: Option<PathBuf>,
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

    /// Spawns `exe` if the path exists. Missing official VRM / missing binary
    /// is [`BodyStatus::Absent`], not a fake overlay.
    pub fn spawn_if_present(&mut self, exe: &Path) -> BodyStatus {
        if !exe.is_file() {
            self.exe = Some(exe.to_path_buf());
            return BodyStatus::Absent;
        }
        self.shutdown();
        match Command::new(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.child = Some(child);
                self.exe = Some(exe.to_path_buf());
                BodyStatus::Spawned
            }
            Err(_) => BodyStatus::Absent,
        }
    }

    pub fn poll(&mut self) -> BodyStatus {
        match self.child.as_mut() {
            None => BodyStatus::Absent,
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    BodyStatus::Exited
                }
                Ok(None) => BodyStatus::Spawned,
                Err(_) => BodyStatus::Exited,
            },
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.kill() {
                Ok(()) | Err(_) => {}
            }
            match child.wait() {
                Ok(_) | Err(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BodyStatus, BodySupervisor};
    use std::path::Path;

    #[test]
    fn missing_binary_is_absent_not_a_character() {
        let mut supervisor = BodySupervisor::new();
        let status = supervisor.spawn_if_present(Path::new("/no/such/ene-body"));
        assert_eq!(status, BodyStatus::Absent);
        assert_eq!(supervisor.poll(), BodyStatus::Absent);
    }
}
