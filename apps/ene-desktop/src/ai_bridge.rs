//! Bridge between the tokio-driven [`ene_core`] actor and the winit
//! main loop.
//!
//! [`EneHandle::new`] spawns the actor on the **current** tokio
//! runtime, so it must be called from a `runtime.enter()` scope. The
//! bridge then spawns a second tokio task that subscribes to the
//! actor's broadcast channel and maps every [`EneEvent`] into a
//! flattened [`AiStreamUpdate`] / [`EmoteToken`], pushing them into
//! the cross-subsystem [`AppEventBus`](crate::events).
//!
//! The winit runtime never blocks on the actor; user input is
//! delivered via [`AiBridge::run`] which is a fire-and-forget mpsc
//! send (the actor's `EneCommand::Run` is unbounded, so the send
//! cannot block).
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ene_core::{EneEvent, EneEventReceiver, EneHandle, PermissionDecision, UserInputResponse};

use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};

/// Owns the actor handle. The runtime can also send user input
/// back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    /// Set on `EneCommand::Run`, cleared on
    /// `EneEvent::Done` / `EneEvent::Failed`. The AI page's chat
    /// input and Send button read this via
    /// [`AiBridge::is_processing`] and wrap themselves in
    /// `ui.disable()` while a request is in flight.
    processing: Arc<AtomicBool>,
}

impl AiBridge {
    /// Build a new bridge and spawn the background drain task. Must
    /// be called from inside `tokio::runtime::Handle::current()`.
    ///
    /// The `event_tx` sender is cloned into the background task; the
    /// receiver is held by the runtime.
    pub fn new(event_tx: AppEventSender, bootstrap_handle: &tokio::runtime::Handle) -> Self {
        let handle = EneHandle::new();
        let receiver = handle.subscribe();
        let processing = Arc::new(AtomicBool::new(false));
        let bridge = Self {
            handle: handle.clone(),
            processing: processing.clone(),
        };

        // Background drain: EneEvent -> AppEvent
        bootstrap_handle.spawn(pump_events(receiver, event_tx.clone(), processing.clone()));

        // Background bootstrap: load_config + load_character
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

    /// Send a `Run` command. Also sets the `processing` flag
    /// to `true` so the AI page can disable the chat input until
    /// the actor reports `Done` / `Failed`. A failed send
    /// (e.g. the actor's command channel is closed) immediately
    /// clears the flag so a broken connection doesn't
    /// permanently lock the UI.
    pub fn run(&self, input: impl Into<String>) {
        self.processing.store(true, Ordering::Relaxed);
        if let Err(e) = self.handle.run(input) {
            tracing::warn!("[Ene] Failed to send Run command: {e}");
            self.processing.store(false, Ordering::Relaxed);
        }
    }

    /// Forward a cancel command.
    #[expect(dead_code)]
    pub fn cancel(&self) {
        if let Err(e) = self.handle.cancel() {
            tracing::warn!("[Ene] Failed to send Cancel command: {e}");
        }
    }

    /// Returns `true` while a request is in flight (i.e.
    /// between the `Run` send and the matching `Done` /
    /// `Failed`). The AI page's chat input and Send button
    /// gate on this.
    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Relaxed)
    }

    /// Forward a `PermissionDecision` for the request
    /// currently sitting in `UiState::pending_permission`.
    pub fn answer_permission(
        &self,
        request_id: impl Into<ene_core::RequestId>,
        decision: PermissionDecision,
    ) -> Result<(), String> {
        self.handle
            .decide_permission(request_id, decision)
            .map_err(|e| e.to_string())
    }

    /// Forward a `UserInputResponse` for the request
    /// currently sitting in `UiState::pending_user_input`.
    pub fn answer_user_input(
        &self,
        request_id: impl Into<ene_core::RequestId>,
        response: UserInputResponse,
    ) -> Result<(), String> {
        self.handle
            .submit_user_input(request_id, response)
            .map_err(|e| e.to_string())
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
            Ok(EneEvent::SessionSplit { .. }) => {}
            Ok(EneEvent::Terminal(ene_core::TerminalReason::Done))
            | Ok(EneEvent::Terminal(ene_core::TerminalReason::Cancelled)) => {
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished));
            }
            Ok(EneEvent::Terminal(ene_core::TerminalReason::Failed { message })) => {
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(message)));
            }
            Ok(EneEvent::StatusChanged { .. }) => {}
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

    /// AtomicBool round-trip.
    #[test]
    fn processing_flag_round_trip() {
        let processing = Arc::new(AtomicBool::new(false));
        processing.store(true, Ordering::Relaxed);
        assert!(processing.load(Ordering::Relaxed));
        processing.store(false, Ordering::Relaxed);
        assert!(!processing.load(Ordering::Relaxed));
    }

    /// `Failed` events clear the processing flag.
    #[test]
    fn processing_flag_clears_on_failed() {
        let processing = Arc::new(AtomicBool::new(true));
        processing.store(false, Ordering::Relaxed);
        assert!(!processing.load(Ordering::Relaxed));
    }
}
