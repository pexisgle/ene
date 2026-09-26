use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ene_body::ipc::{
    BodyToParent, IpcError, LocalUiFact, ParentToBody, PresentationFeedback, decode_body,
    encode_parent, frame_len,
};

const EVENT_QUEUE_CAPACITY: usize = 64;
const COMMAND_QUEUE_CAPACITY: usize = 16;
const EVENT_DRAIN_BATCH: usize = 64;
const LOCAL_UI_QUEUE_CAPACITY: usize = 64;
const PRESENTATION_QUEUE_CAPACITY: usize = 64;
const SHUTDOWN_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStatus {
    Absent,
    Spawned,
    Exited,
}

#[derive(Default)]
pub struct BodySupervisor {
    child: Option<Child>,
    command_tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    events: Option<Receiver<BodyToParent>>,
    event_overflow: Arc<AtomicBool>,
    writer_failed: Arc<AtomicBool>,
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
        let mut command = Command::new(exe);
        command
            .arg("--ipc-stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                let stdout = child.stdout.take();
                let (command_tx, writer, writer_failed) = match stdin.map(|mut stdin| {
                    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(COMMAND_QUEUE_CAPACITY);
                    let writer_failed = Arc::new(AtomicBool::new(false));
                    let thread_failed = Arc::clone(&writer_failed);
                    let writer = thread::spawn(move || {
                        while let Ok(bytes) = rx.recv() {
                            if stdin.write_all(&bytes).is_err() || stdin.flush().is_err() {
                                thread_failed.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    });
                    (tx, writer, writer_failed)
                }) {
                    Some((tx, writer, failed)) => (Some(tx), Some(writer), failed),
                    None => (None, None, Arc::new(AtomicBool::new(false))),
                };
                let event_overflow = Arc::new(AtomicBool::new(false));
                let reader_overflow = Arc::clone(&event_overflow);
                let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
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
                                                match tx.try_send(message) {
                                                    Ok(()) => {}
                                                    Err(mpsc::TrySendError::Full(_)) => {
                                                        reader_overflow
                                                            .store(true, Ordering::SeqCst);
                                                        return;
                                                    }
                                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                                        return;
                                                    }
                                                }
                                            }
                                            Err(IpcError::Truncated { .. }) => break,
                                            Err(_) => match frame_len(&buf) {
                                                Ok(len) => {
                                                    buf.drain(..len);
                                                }
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
                self.command_tx = command_tx;
                self.writer = writer;
                self.reader = reader;
                self.events = Some(rx);
                self.event_overflow = event_overflow;
                self.writer_failed = writer_failed;
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
        if self.writer_failed.load(Ordering::SeqCst) {
            return Err(BodySuperviseError::Write);
        }
        let bytes = encode_parent(message).map_err(|_| BodySuperviseError::Encode)?;
        let sender = self
            .command_tx
            .clone()
            .ok_or(BodySuperviseError::NotRunning)?;
        match sender.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(BodySuperviseError::Backpressure),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.writer_failed.store(true, Ordering::SeqCst);
                Err(BodySuperviseError::NotRunning)
            }
        }
    }

    pub fn poll(&mut self) -> BodyStatus {
        self.drain_events();
        if self.event_overflow.load(Ordering::SeqCst) || self.writer_failed.load(Ordering::SeqCst) {
            self.shutdown();
            return BodyStatus::Exited;
        }
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

    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        self.local_ui.pop_front()
    }

    pub fn take_presentation(&mut self) -> Option<PresentationFeedback> {
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
        self.command_tx.take();
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
            if !reap_child_bounded(&mut child) {
                thread::spawn(move || {
                    drop(child.wait());
                });
            }
        }
        join_if_finished(self.writer.take());
        self.events = None;
        join_if_finished(self.reader.take());
        self.event_overflow.store(false, Ordering::SeqCst);
        self.writer_failed.store(false, Ordering::SeqCst);
        self.local_ui.clear();
        self.presentations.clear();
        self.native_ready = false;
        self.asset_ready = false;
        self.motion_ready = false;
    }

    fn drain_events(&mut self) {
        let mut disconnected = false;
        let mut overflowed = false;
        if let Some(events) = self.events.as_mut() {
            for _ in 0..EVENT_DRAIN_BATCH {
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
                        BodyToParent::AssetFail(_) => self.asset_ready = false,
                        BodyToParent::HealthTick(tick) => {
                            self.motion_ready =
                                tick.motion == ene_body::ipc::FeatureSupport::Available;
                        }
                        BodyToParent::LocalUi(fact) => {
                            if self.local_ui.len() >= LOCAL_UI_QUEUE_CAPACITY {
                                overflowed = true;
                                break;
                            }
                            self.local_ui.push_back(fact);
                        }
                        BodyToParent::Presentation(feedback) => {
                            if self.presentations.len() >= PRESENTATION_QUEUE_CAPACITY {
                                overflowed = true;
                                break;
                            }
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
        if overflowed {
            self.event_overflow.store(true, Ordering::SeqCst);
        }
        if disconnected {
            self.events = None;
        }
    }

    fn drop_io(&mut self) {
        self.command_tx.take();
        self.events = None;
        self.local_ui.clear();
        self.presentations.clear();
        self.native_ready = false;
        self.asset_ready = false;
        self.motion_ready = false;
    }
}

fn reap_child_bounded(child: &mut Child) -> bool {
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

fn join_if_finished(handle: Option<JoinHandle<()>>) {
    if let Some(handle) = handle
        && handle.is_finished()
    {
        drop(handle.join());
    }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child) {
    let pid = i32::try_from(child.id()).unwrap_or(i32::MAX);
    // SAFETY: the child is spawned as the leader of its own process group, so
    // the negative pid targets only that Body process tree.
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
    drop(child.kill());
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) {
    drop(child.kill());
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
    #[error("body projection queue is full")]
    Backpressure,
}
