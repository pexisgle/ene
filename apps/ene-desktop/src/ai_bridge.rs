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
use crate::proactive_observe::{ProactiveObserveControl, spawn_proactive_observer};
use crate::settings::MemoryJournalRecallRow;

/// Payload returned by [`AiBridge::refresh_memory_journal`].
pub struct MemoryJournalPayload {
    pub memories: Vec<ene_store::MemoryItem>,
    pub affect: ene_store::AffectState,
    pub commitments: Vec<ene_store::Commitment>,
    pub pending_writes: usize,
    pub permanent_writes: usize,
}

/// Owns the actor handle. The runtime can also send user input
/// back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    runtime: tokio::runtime::Handle,
    /// Set on run, cleared on Terminal.
    processing: Arc<AtomicBool>,
    /// Active turn id for cancel correlation.
    active_turn: Arc<Mutex<Option<TurnId>>>,
    /// Proactive observation control (#168).
    proactive_observe: ProactiveObserveControl,
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
        let mind = config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
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
        let proactive_observe = spawn_proactive_observer(bootstrap_handle, handle.clone(), &mind);
        let bridge = Self {
            handle: handle.clone(),
            runtime: bootstrap_handle.clone(),
            processing: processing.clone(),
            active_turn: active_turn.clone(),
            proactive_observe,
        };

        bootstrap_handle.spawn(pump_events(
            receiver,
            event_tx,
            handle,
            processing,
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
        self.active_turn.lock().is_ok_and(|g| g.is_some())
    }

    /// Refresh Features-tab settings into the runtime actor (no GGUF reload).
    pub fn sync_feature_runtime(
        &self,
        mind: &ene_mind::MindConfig,
        store: &ene_store::StoreConfig,
        tools: &ene_tool_host::ToolConfig,
        rag: &ene_tool_rag::ToolRagConfig,
    ) {
        self.proactive_observe.apply_mind(mind);
        if let Err(e) = self
            .handle
            .update_feature_settings(ene_runtime::FeatureSettingsUpdate {
                mind: mind.clone(),
                store: store.clone(),
                tools: tools.clone(),
                rag: rag.clone(),
            })
        {
            tracing::warn!(
                component = "AiBridge",
                error = %e,
                "Failed to push feature settings to runtime actor"
            );
        }
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

    /// List the standing session-wide permission grants (#177).
    ///
    /// Blocks the calling thread on the tokio runtime while the actor
    /// answers, mirroring [`AiBridge::get_snapshot_blocking`]. Intended
    /// for the permission-center settings page.
    pub fn list_permissions_blocking(&self) -> Result<Vec<ene_runtime::PermissionScope>, String> {
        self.runtime
            .block_on(self.handle.list_permissions())
            .map_err(|e| e.to_string())
    }

    /// Revoke a single standing permission grant by id (#177).
    ///
    /// Returns whether a grant was actually removed.
    pub fn revoke_permission_blocking(&self, id: u64) -> Result<bool, String> {
        self.runtime
            .block_on(self.handle.revoke_permission(id))
            .map_err(|e| e.to_string())
    }

    /// Revoke every standing permission grant, returning the number
    /// removed (#177).
    pub fn reset_all_permissions_blocking(&self) -> Result<usize, String> {
        self.runtime
            .block_on(self.handle.reset_all_permissions())
            .map_err(|e| e.to_string())
    }

    /// Undo the most recent reversible tool operation (#178).
    ///
    /// Blocks the calling thread on the tokio runtime while the actor
    /// answers, mirroring [`AiBridge::reset_all_permissions_blocking`].
    pub fn undo_blocking(&self) -> Result<ene_runtime::UndoReport, String> {
        self.runtime
            .block_on(self.handle.undo())
            .map_err(|e| e.to_string())
    }

    /// List stored session metadata (#176).
    ///
    /// Blocks the calling thread on the tokio runtime while the actor
    /// answers, mirroring [`AiBridge::list_permissions_blocking`].
    /// Intended for the sessions settings page.
    pub fn list_sessions_blocking(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<ene_store::SessionMeta>, String> {
        self.runtime
            .block_on(self.handle.list_sessions(include_archived, limit))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Export a session as a pretty-printed JSON string (#176).
    pub fn export_session_blocking(&self, session_id: impl Into<String>) -> Result<String, String> {
        self.runtime
            .block_on(self.handle.export_session(session_id))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Import a session from a JSON string, returning the imported
    /// session's row id (#176).
    pub fn import_session_blocking(&self, json: impl Into<String>) -> Result<i64, String> {
        self.runtime
            .block_on(self.handle.import_session(json))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Search session messages, returning matching
    /// `(session_id, message)` pairs (#176).
    pub fn search_sessions_blocking(
        &self,
        query: impl Into<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, ene_store::ExportedMessage)>, String> {
        self.runtime
            .block_on(self.handle.search_sessions(query, limit, offset))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Archive or unarchive a session, returning whether the archived
    /// flag actually changed (#176).
    pub fn archive_session_blocking(
        &self,
        session_id: impl Into<String>,
        archived: bool,
    ) -> Result<bool, String> {
        self.runtime
            .block_on(self.handle.archive_session(session_id, archived))
            .map_err(|e| e.to_string())?
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

    /// Snapshot of cached provider health reports for the AI settings page (#175).
    pub fn provider_health_reports(&self) -> Vec<ene_ai::ProviderHealthReport> {
        self.handle.diagnostics().provider_health_reports()
    }

    /// Snapshot of recent provider fallback events for the AI settings page (#175).
    pub fn provider_fallback_history(&self) -> Vec<ene_ai::FallbackRecord> {
        self.handle.diagnostics().provider_fallback_history()
    }

    /// Run [`ene_ai::validate_api_key`] on the bridge runtime (#241).
    pub fn validate_api_key_blocking(&self, base_url: &str, api_key: &str) -> Result<(), String> {
        self.runtime
            .block_on(ene_ai::validate_api_key(base_url, api_key))
            .map_err(|e| e.to_string())
    }

    /// Refresh memory journal payload (typed memories + affect + commitments).
    pub fn refresh_memory_journal(
        &self,
        limit: usize,
        include_user_deleted: bool,
        include_archived: bool,
        include_superseded: bool,
    ) -> Result<MemoryJournalPayload, String> {
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
            let (pending_writes, permanent_writes) = memory
                .store()
                .ok_or_else(|| "Memory store is not available".to_string())?
                .count_pending_memory_writes(character_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(MemoryJournalPayload {
                memories,
                affect,
                commitments,
                pending_writes,
                permanent_writes,
            })
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
                    .set_memory_status(id, ene_store::MemoryStatus::Archived)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Forget => memory
                    .user_forget_typed_memory(id)
                    .await
                    .map_err(|e| e.to_string()),
                MemoryJournalAction::Dispute => memory
                    .set_memory_status(id, ene_store::MemoryStatus::Disputed)
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
            .block_on(memory.set_memory_status(id, status))
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

const fn cue_source_to_u8(source: ene_runtime::CueSource) -> u8 {
    ene_mind::cue_source_priority(source)
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
            Ok(EneEvent::TextDelta {
                turn,
                origin: _,
                delta,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(delta)));
            }
            Ok(EneEvent::Performance {
                turn,
                origin: _,
                cues,
                source,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                for cue in cues {
                    match cue.kind {
                        ene_mind::PerfKind::Motion => {
                            let layer = cue
                                .motion_layer
                                .map_or_else(|| "full".to_string(), |l| l.as_str().to_string());
                            let priority = cue_source_to_u8(source);
                            let _ = event_tx.send(AppEvent::MotionCue {
                                name: cue.name,
                                layer,
                                priority,
                                duration: 0.0,
                            });
                        }
                        ene_mind::PerfKind::Expression => {
                            let weight = cue.weight.unwrap_or(1.0);
                            let hold_secs = cue.hold_secs.unwrap_or(4.0);
                            let _ = event_tx.send(AppEvent::ExpressionCue {
                                name: cue.name,
                                weight,
                                hold_secs,
                            });
                        }
                        ene_mind::PerfKind::Cancel => {
                            let _ = event_tx.send(AppEvent::CancelCue { scope: cue.name });
                        }
                        ene_mind::PerfKind::LookAt => {
                            let _ = event_tx.send(AppEvent::LookAtCue {
                                target: cue.name.clone(),
                            });
                        }
                    }
                }
            }
            Ok(EneEvent::ToolCallStart {
                turn,
                origin: _,
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
            Ok(EneEvent::ToolCallResult {
                turn,
                origin: _,
                name,
                result,
            }) => {
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
                origin: _,
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
                origin: _,
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
            Ok(EneEvent::ContextCompressed { .. }) => {}
            Ok(EneEvent::StatusChanged { .. } | EneEvent::ToolBackgroundCompleted { .. }) => {}
            Ok(EneEvent::TurnStarted { turn, origin: _ }) => {
                if let Ok(mut guard) = active_turn.lock() {
                    *guard = Some(turn);
                }
                processing.store(true, Ordering::Relaxed);
            }
            Ok(EneEvent::Terminal {
                turn,
                origin: _,
                reason: ene_runtime::TerminalReason::Done | ene_runtime::TerminalReason::Cancelled,
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
                origin: _,
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

    /// `AtomicBool` round-trip.
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
    fn turn_matches_accepts_after_turn_started() {
        let turn = TurnId::new();
        let active = Mutex::new(Some(turn.clone()));
        assert!(turn_matches(&active, &turn));
    }
}
