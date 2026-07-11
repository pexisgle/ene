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

use ene_core::{
    BootstrapOptions, EneEvent, EneEventReceiver, EneHandle, PermissionDecision, UserInputResponse,
    bootstrap_runtime,
};
use ene_config::EneConfig;

use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};
use crate::memory_journal::{MemoryJournalAction, MemoryJournalPresenter};
use crate::settings::MemoryJournalRecallRow;

/// Owns the actor handle. The runtime can also send user input
/// back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    runtime: tokio::runtime::Handle,
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
    ///
    /// `config` must be the same [`EneConfig`] already loaded by
    /// [`crate::settings::CharacterSettings::discover`] so the actor
    /// does not reload settings from disk a second time.
    pub fn new(
        event_tx: AppEventSender,
        bootstrap_handle: &tokio::runtime::Handle,
        config: EneConfig,
    ) -> Self {
        let handle = EneHandle::new();
        let receiver = handle.subscribe();
        let processing = Arc::new(AtomicBool::new(false));
        let bridge = Self {
            handle: handle.clone(),
            runtime: bootstrap_handle.clone(),
            processing: processing.clone(),
        };

        // Background drain: EneEvent -> AppEvent
        bootstrap_handle.spawn(pump_events(receiver, event_tx.clone(), processing.clone()));

        // Phase 3: runtime warmup (reconfigure, character, tool index, CCv3 sync)
        bootstrap_handle.spawn({
            let handle = handle.clone();
            async move {
                if let Err(e) =
                    bootstrap_runtime(&handle, BootstrapOptions::with_config(config)).await
                {
                    tracing::warn!(
                        component = "AiBridge",
                        error = %e,
                        "Runtime bootstrap failed"
                    );
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
    /// currently sitting in `ChatState::pending_permission`.
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
    /// currently sitting in `ChatState::pending_user_input`.
    pub fn answer_user_input(
        &self,
        request_id: impl Into<ene_core::RequestId>,
        response: UserInputResponse,
    ) -> Result<(), String> {
        self.handle
            .submit_user_input(request_id, response)
            .map_err(|e| e.to_string())
    }

    /// Fetches a fresh actor snapshot on the runtime thread.
    pub fn get_snapshot_blocking(&self) -> Result<ene_core::EneStateSnapshot, String> {
        self.runtime
            .block_on(self.handle.get_snapshot())
            .map_err(|e| e.to_string())
    }

    /// Refresh memory journal payload (typed memories + affect + commitments).
    pub fn refresh_memory_journal(
        &self,
        limit: usize,
        include_user_deleted: bool,
        include_archived: bool,
        include_superseded: bool,
    ) -> Result<
        (
            Vec<ene_memory::MemoryItem>,
            ene_memory::AffectState,
            Vec<ene_memory::Commitment>,
        ),
        String,
    > {
        let snapshot = self.get_snapshot_blocking()?;
        let character_id = snapshot.card_name.as_str();
        let user_id = snapshot.config.user_name.clone();
        self.runtime.block_on(async {
            let options = ene_memory::MemoryJournalListOptions {
                character_id,
                user_id: Some(user_id.as_str()),
                include_archived,
                include_superseded,
                include_user_deleted,
                kind: None,
                limit,
                offset: 0,
            };
            let memories = snapshot
                .memory
                .list_journal_memories(&options)
                .await
                .map_err(|e| e.to_string())?;
            let affect = snapshot
                .memory
                .show_affect_state(character_id)
                .await
                .map_err(|e| e.to_string())?;
            let commitments = snapshot
                .memory
                .list_active_commitments(character_id, Some(&user_id), limit)
                .await
                .map_err(|e| e.to_string())?;
            Ok((memories, affect, commitments))
        })
    }

    /// Run explainable recall search for the journal debug mode.
    pub fn search_memory_journal_recall(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryJournalRecallRow>, String> {
        let snapshot = self.get_snapshot_blocking()?;
        let character_id = snapshot.card_name.as_str();
        let user_id = snapshot.config.user_name.as_str();
        self.runtime.block_on(async {
            let recalled = snapshot
                .memory
                .search_typed_memories_explained(character_id, Some(user_id), query, limit)
                .await
                .map_err(|e| e.to_string())?;
            Ok(recalled
                .iter()
                .map(|entry| {
                    let breakdown = &entry.score_breakdown;
                    MemoryJournalPresenter::recall_row(
                        entry.item.id.unwrap_or_default(),
                        entry.item.title.clone(),
                        recall_reason_key(entry.reason),
                        format!(
                            "total={:.3} vector={:.3} lexical={:.3} recency={:.3} salience={:.3} confidence={:.3}",
                            breakdown.total,
                            breakdown.vector_similarity,
                            breakdown.lexical_score,
                            breakdown.recency_score,
                            breakdown.salience,
                            breakdown.confidence,
                        ),
                    )
                })
                .collect())
        })
    }

    /// Execute a journal action using the correct store API.
    pub fn execute_journal_action(
        &self,
        id: i64,
        action: MemoryJournalAction,
    ) -> Result<bool, String> {
        let snapshot = self.get_snapshot_blocking()?;
        self.runtime.block_on(async {
            match action {
                MemoryJournalAction::Pin => snapshot
                    .memory
                    .pin_typed_memory(id, true)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Unpin => snapshot
                    .memory
                    .pin_typed_memory(id, false)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Archive => snapshot
                    .memory
                    .transition_typed_memory_status(id, ene_memory::MemoryStatus::Archived)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Forget => snapshot
                    .memory
                    .user_forget_typed_memory(id)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Dispute => snapshot
                    .memory
                    .transition_typed_memory_status(id, ene_memory::MemoryStatus::Disputed)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Restore => snapshot
                    .memory
                    .user_restore_typed_memory(id)
                    .await
                    .map_err(|e| e.to_string()),
            }
        })
    }

    /// Applies a memory lifecycle action.
    #[expect(dead_code, reason = "retained for transitional callers")]
    pub fn update_memory_status(
        &self,
        id: i64,
        status: ene_memory::MemoryStatus,
    ) -> Result<bool, String> {
        let snapshot = self.get_snapshot_blocking()?;
        self.runtime
            .block_on(snapshot.memory.transition_typed_memory_status(id, status))
            .map_err(|e| e.to_string())
    }

    /// Pins a memory row in the typed store.
    #[expect(dead_code, reason = "replaced by execute_journal_action")]
    pub fn pin_memory(&self, id: i64) -> Result<bool, String> {
        let snapshot = self.get_snapshot_blocking()?;
        self.runtime
            .block_on(snapshot.memory.pin_typed_memory(id, true))
            .map_err(|e| e.to_string())
    }
}

fn recall_reason_key(reason: ene_cognition::RecallReason) -> String {
    match reason {
        ene_cognition::RecallReason::SimilarTopic => "similar_topic".to_string(),
        ene_cognition::RecallReason::RecentConversation => "recent_conversation".to_string(),
        ene_cognition::RecallReason::ActivePromise => "active_promise".to_string(),
        ene_cognition::RecallReason::CharacterLore => "character_lore".to_string(),
        ene_cognition::RecallReason::UserPreference => "user_preference".to_string(),
        ene_cognition::RecallReason::EmotionalContinuity => "emotional_continuity".to_string(),
        ene_cognition::RecallReason::Pinned => "pinned".to_string(),
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
            Ok(EneEvent::Expression { name, .. }) => {
                let _ = event_tx.send(AppEvent::EmoteToken(name));
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
            Ok(EneEvent::StatusChanged { .. })
            | Ok(EneEvent::PipelinePhase { .. })
            | Ok(EneEvent::PipelineMetrics { .. }) => {}
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
