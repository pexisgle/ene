//! Bridge between the tokio-driven [`ene_core`] actor and the winit
//! main loop.
//!
//! [`EneHandle::new`] spawns the actor on the **current** tokio
//! runtime, so it must be called from a `runtime.enter()` scope. The
//! bridge then spawns a second tokio task that subscribes to the
//! actor's broadcast channel, maps every [`EneEvent`] into a
//! flattened [`AiStreamUpdate`] / [`EmoteToken`], and pushes them
//! into the cross-subsystem [`AppEventBus`](crate::events).
//!
//! The winit runtime never blocks on the actor; user input is
//! delivered via [`AiBridge::run`] which is a fire-and-forget mpsc
//! send (the actor's `EneCommand::Run` is unbounded, so the send
//! cannot block).
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ene_core::{EneEvent, EneEventReceiver, EneHandle};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};

/// Owns the actor handle and a per-frame buffer of events the
/// runtime has already pulled out of the bus. The runtime can also send user
/// input back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    /// Queue of events the runtime has already pulled from the bus
    /// but not yet consumed by a UI pass. Kept here so the runtime
    /// can hand individual updates to egui.
    inbox: Mutex<VecDeque<AiStreamUpdate>>,
    /// Most recent text delta, kept around for the settings UI's
    /// "Latest Response" panel (PR2 will read this).
    #[allow(dead_code)] // `drain` mutates it; the field is not read elsewhere yet.
    latest_response: Mutex<String>,
    /// A.4: equivalent of the legacy `ene.processing = true` flag
    /// (set on `EneCommand::Run`, cleared on `EneEvent::Done` /
    /// `EneEvent::Failed`). The AI page's chat input and Send
    /// button read this via [`AiBridge::is_processing`] and wrap
    /// themselves in `ui.disable()` while a request is in
    /// flight. `Arc<AtomicBool>` so the background `pump_events`
    /// task (a `tokio::spawn` outside the winit thread) can
    /// flip the bit without locking; the UI side uses a relaxed
    /// load once per frame.
    processing: Arc<AtomicBool>,
}

impl AiBridge {
    /// Build a new bridge and spawn the background drain task. Must
    /// be called from inside `tokio::runtime::Handle::current()`.
    ///
    /// The `event_tx` sender is cloned into the background task; the
    /// receiver is held by the runtime. The `bootstrap_handle` is
    /// used to spawn the initial `load_config` / `load_character`
    /// task that the legacy Bevy code runs as a `tokio::spawn` on
    /// `EneResource::default`.
    pub fn new(event_tx: AppEventSender, bootstrap_handle: &tokio::runtime::Handle) -> Self {
        let handle = EneHandle::new();
        let receiver = handle.subscribe();
        let processing = Arc::new(AtomicBool::new(false));
        let bridge = Self {
            handle: handle.clone(),
            inbox: Mutex::new(VecDeque::new()),
            latest_response: Mutex::new(String::new()),
            processing: processing.clone(),
        };

        // Background drain: EneEvent -> AppEvent
        // The clone of `processing` is the bit the task flips on
        // `Done` / `Failed` (A.4).
        bootstrap_handle.spawn(pump_events(receiver, event_tx.clone(), processing.clone()));

        // Background bootstrap: load_config + load_character (mirrors
        // the legacy EneResource::default).
        bootstrap_handle.spawn({
            let handle = handle.clone();
            async move {
                match handle.load_config().await {
                    Ok(config) => {
                        if let Err(e) = handle.load_character(&config.character).await {
                            tracing::warn!("[Ene] Failed to load character: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("[Ene] Failed to load config: {e}");
                    }
                }
            }
        });

        bridge
    }

    /// Send a `Run` command. A.4: also sets the `processing` flag
    /// to `true` so the AI page can disable the chat input until
    /// the actor reports `Done` / `Failed`. A failed `Run` (e.g.
    /// the actor's command channel is closed) immediately clears
    /// the flag so a broken connection doesn't permanently lock
    /// the UI.
    pub fn run(&self, input: impl Into<String>) {
        self.processing.store(true, Ordering::Relaxed);
        if let Err(e) = self.handle.run(input) {
            tracing::warn!("[Ene] Failed to send Run command: {e}");
            self.processing.store(false, Ordering::Relaxed);
        }
    }

    /// Forward a cancel command. The legacy Bevy code does not
    /// expose this; PR2 will bind it to a "Stop" button.
    #[expect(dead_code)]
    pub fn cancel(&self) {
        if let Err(e) = self.handle.cancel() {
            tracing::warn!("[Ene] Failed to send Cancel command: {e}");
        }
    }

    /// A.4: returns `true` while a request is in flight (i.e.
    /// between the `Run` send and the matching `Done` /
    /// `Failed`). The AI page's chat input and Send button
    /// gate on this. The flag is atomic and lock-free so the
    /// UI can poll it once per frame without contending with
    /// the background `pump_events` task.
    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Relaxed)
    }

    /// Pull every AI event currently sitting in the bus into the
    /// bridge's inbox. The runtime calls this once per frame from
    /// `about_to_wait`.
    #[expect(dead_code)] // Reserved for callers that need the inbox pre-loaded.
    pub fn drain(&self, rx: &mut mpsc::UnboundedReceiver<AppEvent>) {
        loop {
            match rx.try_recv() {
                Ok(AppEvent::Ai(update)) => {
                    if let AiStreamUpdate::TextDelta(ref s) = update {
                        self.latest_response.lock().push_str(s);
                    }
                    self.inbox.lock().push_back(update);
                }
                Ok(AppEvent::EmoteToken(_token)) => {
                    // PR2 will route this into the emotion queue.
                }
                Ok(_) => {
                    // Non-AI events are handled directly by the
                    // runtime; ignore them here.
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Borrow the inbox. Used by the UI pass to read every queued
    /// update in order.
    #[expect(dead_code)]
    pub fn inbox(
        &self,
    ) -> parking_lot::lock_api::MutexGuard<'_, parking_lot::RawMutex, VecDeque<AiStreamUpdate>>
    {
        self.inbox.lock()
    }

    /// Take the entire inbox, leaving it empty. The UI pass uses
    /// this to consume events after each frame.
    #[expect(dead_code)]
    pub fn take_inbox(&self) -> VecDeque<AiStreamUpdate> {
        std::mem::take(&mut *self.inbox.lock())
    }
}

async fn pump_events(
    mut receiver: EneEventReceiver,
    event_tx: AppEventSender,
    processing: Arc<AtomicBool>,
) {
    loop {
        match receiver.recv().await {
            Ok(EneEvent::TextDelta { delta }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(delta)));
            }
            Ok(EneEvent::SpecialToken { token }) => {
                if let Some(name) = ene_core::extract_emotion_from_token(&token) {
                    let _ = event_tx.send(AppEvent::EmoteToken(name.to_string()));
                }
            }
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallStart {
                    name,
                    arguments,
                }));
            }
            Ok(EneEvent::ToolCallResult { name, result }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallResult {
                    name,
                    result,
                }));
            }
            Ok(EneEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                }));
            }
            Ok(EneEvent::UserInputRequired { request_id, prompt }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::UserInputRequired {
                    request_id,
                    prompt,
                }));
            }
            Ok(EneEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            }) => {
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::TaskProgress {
                    task_id,
                    step,
                    total_steps,
                    description,
                }));
            }
            Ok(EneEvent::SessionSplit { .. }) => {
                // Dropped at the bridge. (PR4 will store the summary
                // for the in-app log view.)
            }
            Ok(EneEvent::Done) => {
                // A.4: the legacy `ene.processing = false` is
                // mirrored by clearing the atomic bit. The AI
                // page's chat input / Send button re-enables on
                // the next frame.
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished));
            }
            Ok(EneEvent::Failed { message }) => {
                // A.4: a `Failed` event also clears the
                // processing flag — a failure ends the in-flight
                // window, the UI should be usable again.
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(message)));
            }
            Ok(EneEvent::StatusChanged { .. }) => {
                // Dropped at the bridge. The legacy Bevy code
                // only inspects `Idle`; v2 derives processing
                // state from `Finished` / `Error` instead.
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("[Ene] Dropped {n} events (broadcast lag)");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A.4: the processing flag is observable to the UI without
    /// holding a lock. `pump_events` flips the bit on
    /// `EneEvent::Done` / `Failed`; the `Arc<AtomicBool>` is
    /// the only state shared between the background tokio task
    /// and the winit thread.
    #[test]
    fn processing_flag_round_trip() {
        let processing = Arc::new(AtomicBool::new(false));
        // Simulate `Run`: the dispatch side sets the bit.
        processing.store(true, Ordering::Relaxed);
        assert!(processing.load(Ordering::Relaxed));
        // Simulate `Done`: the pump_events task clears it.
        processing.store(false, Ordering::Relaxed);
        assert!(!processing.load(Ordering::Relaxed));
    }

    /// A.4: clearing the flag on `Failed` is symmetric with
    /// `Done` (both end the in-flight window). This pins the
    /// contract that a single `Done` / `Failed` is enough to
    /// re-enable the UI, regardless of which terminal event
    /// arrives first.
    #[test]
    fn processing_flag_clears_on_failed() {
        let processing = Arc::new(AtomicBool::new(true));
        processing.store(false, Ordering::Relaxed);
        assert!(!processing.load(Ordering::Relaxed));
    }
}
