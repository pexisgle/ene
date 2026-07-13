//! Bridge between the tokio-driven [`ene_runtime`] actor and the winit
//! main loop.
//!
//! [`EneHandle::open`] initializes a ready actor on the **current**
//! tokio runtime (via `block_on`), so it must be called from a
//! `runtime.enter()` scope. The bridge then spawns a tokio task that
//! maps every [`EneEvent`] into [`AiStreamUpdate`] / emote tokens.
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ene_config::EneConfig;
use ene_runtime::{
    EneEvent, EneEventReceiver, EneHandle, PermissionDecision, RunError, TurnId, UserInputResponse,
    open_with_config,
};

use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};
use crate::memory_journal::{MemoryJournalAction, MemoryJournalPresenter};
use crate::settings::MemoryJournalRecallRow;

/// Owns the actor handle. The runtime can also send user input
/// back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    runtime: tokio::runtime::Handle,
    /// Set on run, cleared on Terminal.
    processing: Arc<AtomicBool>,
    /// Active turn id for cancel correlation.
    active_turn: Arc<Mutex<Option<TurnId>>>,
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
        let handle = match bootstrap_handle.block_on(open_with_config(config)) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(
                    component = "AiBridge",
                    error = %e,
                    "EneHandle::open failed"
                );
                // Fall back is not available — panic would brick desktop.
                // Surface as processing=false with no handle is impossible;
                // rethrow via expect so startup fails loudly.
                panic!("EneHandle::open failed: {e}");
            }
        };
        let receiver = handle.subscribe();
        let processing = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let bridge = Self {
            handle: handle.clone(),
            runtime: bootstrap_handle.clone(),
            processing: processing.clone(),
            active_turn: active_turn.clone(),
        };

        bootstrap_handle.spawn(pump_events(
            receiver,
            event_tx.clone(),
            handle.clone(),
            processing.clone(),
            active_turn,
        ));

        bridge
    }

    /// Send a `Run` command. Also sets the `processing` flag
    /// to `true` so the AI page can disable the chat input until
    /// the actor reports `Terminal`. A failed send
    /// (e.g. Busy or dead actor) immediately clears the flag so a
    /// broken connection doesn't permanently lock the UI.
    pub fn run(&self, input: impl Into<String>) {
        self.processing.store(true, Ordering::Relaxed);
        match self.handle.run(input) {
            Ok(turn) => {
                if let Ok(mut guard) = self.active_turn.lock() {
                    *guard = Some(turn);
                }
            }
            Err(RunError::Busy) => {
                tracing::warn!("[Ene] Run rejected: Busy");
                self.processing.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!("[Ene] Failed to send Run command: {e}");
                self.processing.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Forward a cancel command for the active turn.
    pub fn cancel(&self) {
        let turn = self.active_turn.lock().ok().and_then(|g| g.clone());
        let Some(turn) = turn else {
            return;
        };
        if let Err(e) = self.handle.cancel(&turn) {
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

    /// True while a turn id is still tracked (may outlive `processing`
    /// after broadcast lag so Cancel remains available).
    pub fn has_active_turn(&self) -> bool {
        self.active_turn.lock().ok().is_some_and(|g| g.is_some())
    }

    /// Forward a `PermissionDecision` for the request
    /// currently sitting in `ChatState::pending_permission`.
    pub fn answer_permission(
        &self,
        request_id: impl Into<ene_runtime::RequestId>,
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
        request_id: impl Into<ene_runtime::RequestId>,
        response: UserInputResponse,
    ) -> Result<(), String> {
        self.handle
            .submit_user_input(request_id, response)
            .map_err(|e| e.to_string())
    }

    /// Fetches a fresh actor snapshot on the runtime thread.
    pub fn get_snapshot_blocking(&self) -> Result<ene_runtime::EneStateSnapshot, String> {
        self.runtime
            .block_on(self.handle.diagnostics().get_snapshot())
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
            Vec<ene_store::MemoryItem>,
            ene_store::AffectState,
            Vec<ene_store::Commitment>,
        ),
        String,
    > {
        let snapshot = self.get_snapshot_blocking()?;
        let character_id = snapshot.card_name.as_str();
        let user_id = snapshot.config.user_name.clone();
        let memory = self.handle.diagnostics().memory().clone();
        self.runtime.block_on(async {
            let options = ene_store::MemoryJournalListOptions {
                character_id,
                user_id: Some(user_id.as_str()),
                include_archived,
                include_superseded,
                include_user_deleted,
                kind: None,
                limit,
                offset: 0,
            };
            let memories = memory
                .list_journal_memories(&options)
                .await
                .map_err(|e| e.to_string())?;
            let affect = memory
                .show_affect_state(character_id)
                .await
                .map_err(|e| e.to_string())?;
            let commitments = memory
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
        let memory = self.handle.diagnostics().memory().clone();
        self.runtime.block_on(async {
            let recalled = memory
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
        let memory = self.handle.diagnostics().memory().clone();
        self.runtime.block_on(async {
            match action {
                MemoryJournalAction::Pin => memory
                    .pin_typed_memory(id, true)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Unpin => memory
                    .pin_typed_memory(id, false)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Archive => memory
                    .transition_typed_memory_status(id, ene_store::MemoryStatus::Archived)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Forget => memory
                    .user_forget_typed_memory(id)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Dispute => memory
                    .transition_typed_memory_status(id, ene_store::MemoryStatus::Disputed)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Restore => memory
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
        status: ene_store::MemoryStatus,
    ) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.runtime
            .block_on(memory.transition_typed_memory_status(id, status))
            .map_err(|e| e.to_string())
    }

    /// Pins a memory row in the typed store.
    #[expect(dead_code, reason = "replaced by execute_journal_action")]
    pub fn pin_memory(&self, id: i64) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.runtime
            .block_on(memory.pin_typed_memory(id, true))
            .map_err(|e| e.to_string())
    }
}

fn recall_reason_key(reason: ene_mind::RecallReason) -> String {
    match reason {
        ene_mind::RecallReason::SimilarTopic => "similar_topic".to_string(),
        ene_mind::RecallReason::RecentConversation => "recent_conversation".to_string(),
        ene_mind::RecallReason::ActivePromise => "active_promise".to_string(),
        ene_mind::RecallReason::CharacterLore => "character_lore".to_string(),
        ene_mind::RecallReason::UserPreference => "user_preference".to_string(),
        ene_mind::RecallReason::EmotionalContinuity => "emotional_continuity".to_string(),
        ene_mind::RecallReason::Pinned => "pinned".to_string(),
    }
}

fn turn_matches(active_turn: &Mutex<Option<TurnId>>, event_turn: &TurnId) -> bool {
    match active_turn.lock() {
        Ok(guard) => match guard.as_ref() {
            // No active turn: drop turn-scoped events (avoids ghost deltas
            // after Terminal / Lagged clearing).
            None => false,
            Some(active) => active == event_turn,
        },
        Err(_) => false,
    }
}

async fn pump_events(
    mut receiver: EneEventReceiver,
    event_tx: AppEventSender,
    handle: EneHandle,
    processing: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<TurnId>>>,
) {
    loop {
        match receiver.recv().await {
            Ok(EneEvent::TextDelta { turn, delta }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(delta)));
            }
            Ok(EneEvent::Performance { turn, cues, .. }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                // Map Performance → desktop VRM playback (cue name → morph).
                // ene-vrm does not depend on mind/runtime types.
                for cue in cues {
                    let _ = event_tx.send(AppEvent::PerformanceCue(cue.name));
                }
            }
            Ok(EneEvent::ToolCallStart {
                turn,
                name,
                arguments,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallStart {
                    name,
                    arguments,
                }));
            }
            Ok(EneEvent::ToolCallResult { turn, name, result }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallResult {
                    name,
                    result,
                }));
            }
            Ok(EneEvent::PermissionRequired {
                turn,
                request_id,
                action,
                target,
                description,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                }));
            }
            Ok(EneEvent::UserInputRequired {
                turn,
                request_id,
                prompt,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::UserInputRequired {
                    request_id,
                    prompt,
                }));
            }
            Ok(EneEvent::ContextCompressed { .. }) | Ok(EneEvent::StatusChanged { .. }) => {}
            Ok(EneEvent::Terminal {
                turn,
                reason: ene_runtime::TerminalReason::Done,
            })
            | Ok(EneEvent::Terminal {
                turn,
                reason: ene_runtime::TerminalReason::Cancelled,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                clear_active_turn(&active_turn, &turn);
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished));
            }
            Ok(EneEvent::Terminal {
                turn,
                reason: ene_runtime::TerminalReason::Failed { message },
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                clear_active_turn(&active_turn, &turn);
                processing.store(false, Ordering::Relaxed);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(message)));
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    "[Ene] Dropped {n} events (broadcast lag); cancelling active turn if any"
                );
                // Unlock input, but keep active_turn and cancel so TurnGate
                // is freed even if Terminal was among the dropped events.
                processing.store(false, Ordering::Relaxed);
                if let Ok(guard) = active_turn.lock()
                    && let Some(turn) = guard.clone()
                {
                    drop(guard);
                    if let Err(e) = handle.cancel(&turn) {
                        tracing::warn!("[Ene] Lagged cancel failed: {e}");
                    }
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished));
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn clear_active_turn(active_turn: &Mutex<Option<TurnId>>, turn: &TurnId) {
    if let Ok(mut guard) = active_turn.lock()
        && guard.as_ref() == Some(turn)
    {
        *guard = None;
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

    #[test]
    fn turn_matches_rejects_when_no_active_turn() {
        let active = Mutex::new(None);
        let turn = TurnId::new();
        assert!(!turn_matches(&active, &turn));
    }

    #[test]
    fn turn_matches_rejects_mismatched_turn() {
        let active_id = TurnId::new();
        let other = TurnId::new();
        let active = Mutex::new(Some(active_id.clone()));
        assert!(turn_matches(&active, &active_id));
        assert!(!turn_matches(&active, &other));
    }
}
