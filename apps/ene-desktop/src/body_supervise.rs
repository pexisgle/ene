use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use ene_body::ipc::{
    BodyToParent, IpcError, LocalUiFact, ParentToBody, PresentationFeedback, decode_body,
    encode_parent, frame_len,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStatus {
    Absent,
    Spawned,
    Exited,
}

#[derive(Default)]
pub struct BodySupervisor {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    events: Option<Receiver<BodyToParent>>,
    local_ui: VecDeque<LocalUiFact>,
    presentations: VecDeque<PresentationFeedback>,
    native_ready: bool,
    asset_ready: bool,
    motion_ready: bool,
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

    pub fn spawn_if_present(&mut self, exe: &Path) -> BodyStatus {
        if !exe.is_file() {
            return BodyStatus::Absent;
        }
        self.shutdown();
        match Command::new(exe)
            .arg("--ipc-stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
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
                                            Err(IpcError::Truncated { .. }) => break,
                                            Err(_) => match frame_len(&buf) {
                                                Ok(len) => {
                                                    buf.drain(..len);
                                                }
                                                // The length prefix itself is
                                                // unreadable or oversize: no
                                                // valid boundary exists, so
                                                // dropping only 4 bytes would
                                                // reread body bytes as the next
                                                // length and desynchronize the
                                                // stream. Abort the reader.
                                                Err(_) => return,
                                            },
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
                self.local_ui.clear();
                self.presentations.clear();
                self.native_ready = false;
                self.asset_ready = false;
                self.motion_ready = false;
                BodyStatus::Spawned
            }
            Err(_) => BodyStatus::Absent,
        }
    }

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

    /// Takes an overlay-local settings candidate.
    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        self.drain_events();
        self.local_ui.pop_front()
    }

    pub fn take_presentation(&mut self) -> Option<PresentationFeedback> {
        self.drain_events();
        self.presentations.pop_front()
    }

    #[must_use]
    pub fn available(&self) -> bool {
        self.native_ready && self.asset_ready && self.child.is_some()
    }

    #[must_use]
    pub fn motion_ready(&mut self) -> bool {
        self.drain_events();
        self.motion_ready
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
        if let Some(events) = self.events.as_mut() {
            loop {
                match events.try_recv() {
                    Ok(message) => match message {
                        BodyToParent::Ready(info) => {
                            self.native_ready = info.overlay
                                != ene_body::ipc::OverlayKind::Headless
                                && info.gpu == ene_body::ipc::GpuInitStatus::Ok;
                        }
                        BodyToParent::GpuFail(_) | BodyToParent::OverlayUnavailable(_) => {
                            self.native_ready = false;
                        }
                        BodyToParent::AssetReady(_) => self.asset_ready = true,
                        BodyToParent::HealthTick(tick) => {
                            self.motion_ready =
                                tick.motion == ene_body::ipc::FeatureSupport::Available;
                        }
                        BodyToParent::LocalUi(fact) => self.local_ui.push_back(fact),
                        BodyToParent::Presentation(feedback) => {
                            self.presentations.push_back(feedback);
                        }
                        _ => {}
                    },
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.events = None;
        }
    }

    fn drop_io(&mut self) {
        self.stdin = None;
        self.events = None;
        self.native_ready = false;
        self.asset_ready = false;
        self.motion_ready = false;
    }
}

fn body_binary_name() -> &'static str {
    if cfg!(windows) {
        "ene-body.exe"
    } else {
        "ene-body"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BodySuperviseError {
    #[error("body is not running")]
    NotRunning,
    #[error("projection encode failed")]
    Encode,
    #[error("projection write failed")]
    Write,
}
