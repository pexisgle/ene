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
use std::time::Duration;

use ene_config::EneConfig;
use ene_runtime::{
    EneEvent, EneEventReceiver, EneHandle, EneRuntimeError, PermissionDecision, RunError, TurnId,
    UserInputResponse, open_with_config,
};

#[cfg(not(feature = "voice"))]
use crate::audio::AudioChunkSender;
#[cfg(feature = "voice")]
use crate::audio::{AudioChunkPayload, AudioChunkSender};
use crate::events::{AiStreamUpdate, AppEvent, AppEventSender};
use crate::memory_journal::{MemoryJournalAction, MemoryJournalPresenter};
use crate::proactive_observe::{ProactiveObserveControl, spawn_proactive_observer};
use crate::settings::MemoryJournalRecallRow;
use ene_runtime::handle::PendingCandidateSummary;

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

/// Maximum time a `*_blocking` call may hold the winit main thread
/// before it is cancelled and an error is returned to the UI.
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(10);

/// Converts the API v1 [`ene_runtime::PublicSessionMeta`] DTO back into the
/// `ene_store::SessionMeta` shape the desktop settings UI (#176) already
/// consumes directly.
///
/// `EneHandle::list_sessions` returns the DTO as of #269 so the runtime's
/// public contract doesn't leak `ene_store` types; converting back here (a
/// trivial field copy — both types share the same fields) keeps
/// [`AiBridge::list_sessions_blocking`]'s own `Result<_, String>` signature
/// unchanged, since reworking that boundary is #274's job, not #269's.
fn session_meta_from_public(m: ene_runtime::PublicSessionMeta) -> ene_store::SessionMeta {
    ene_store::SessionMeta {
        id: m.id,
        session_id: m.session_id,
        card_name: m.card_name,
        title: m.title,
        created_at: m.created_at,
        updated_at: m.updated_at,
        archived: m.archived,
        turn_count: m.turn_count,
    }
}

/// Converts the API v1 [`ene_runtime::PublicExportedMessage`] DTO back into
/// `ene_store::ExportedMessage`, mirroring [`session_meta_from_public`].
fn exported_message_from_public(
    m: ene_runtime::PublicExportedMessage,
) -> ene_store::ExportedMessage {
    ene_store::ExportedMessage {
        role: m.role,
        content: m.content,
        created_at: m.created_at,
    }
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
    /// Open the runtime actor and spawn the background event pump.
    pub fn try_new(
        event_tx: AppEventSender,
        bootstrap_handle: &tokio::runtime::Handle,
        config: EneConfig,
        audio_tx: Option<AudioChunkSender>,
    ) -> Result<Self, EneRuntimeError> {
        let mind = config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let handle = bootstrap_handle.block_on(open_with_config(config))?;
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
            audio_tx,
        ));

        Ok(bridge)
    }

    /// Run a future on the bridge runtime with [`BLOCKING_TIMEOUT`].
    ///
    /// Prevents indefinite UI freezes when the actor is slow or
    /// deadlocked: the future is cancelled after the timeout and an
    /// error string is returned to the caller.
    fn block_on_timeout<F, T>(&self, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime
            .block_on(tokio::time::timeout(BLOCKING_TIMEOUT, fut))
            .map_err(|_| format!("Operation timed out after {}s", BLOCKING_TIMEOUT.as_secs()))
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
        plugins: &ene_plugin_host::PluginConfig,
        rag: &ene_tool_rag::ToolRagConfig,
    ) {
        self.proactive_observe.apply_mind(mind);
        if let Err(e) = self
            .handle
            .update_feature_settings(ene_runtime::FeatureSettingsUpdate {
                mind: mind.clone(),
                store: store.clone(),
                plugins: plugins.clone(),
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
        self.block_on_timeout(self.handle.list_permissions())
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Revoke a single standing permission grant by id (#177).
    ///
    /// Returns whether a grant was actually removed.
    pub fn revoke_permission_blocking(&self, id: u64) -> Result<bool, String> {
        self.block_on_timeout(self.handle.revoke_permission(id))
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Revoke every standing permission grant, returning the number
    /// removed (#177).
    pub fn reset_all_permissions_blocking(&self) -> Result<usize, String> {
        self.block_on_timeout(self.handle.reset_all_permissions())
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Undo the most recent reversible tool operation (#178).
    ///
    /// Blocks the calling thread on the tokio runtime while the actor
    /// answers, mirroring [`AiBridge::reset_all_permissions_blocking`].
    pub fn undo_blocking(&self) -> Result<ene_runtime::UndoReport, String> {
        self.block_on_timeout(self.handle.undo())
            .and_then(|r| r.map_err(|e| e.to_string()))
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
        self.block_on_timeout(self.handle.list_sessions(include_archived, limit))
            .and_then(|r| r.map_err(|e| e.to_string()))
            .map(|metas| metas.into_iter().map(session_meta_from_public).collect())
    }

    /// Export a session as a pretty-printed JSON string (#176).
    pub fn export_session_blocking(&self, session_id: impl Into<String>) -> Result<String, String> {
        self.block_on_timeout(self.handle.export_session(session_id))
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Import a session from a JSON string, returning the imported
    /// session's row id (#176).
    pub fn import_session_blocking(&self, json: impl Into<String>) -> Result<i64, String> {
        self.block_on_timeout(self.handle.import_session(json))
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Search session messages, returning matching
    /// `(session_id, message)` pairs (#176).
    pub fn search_sessions_blocking(
        &self,
        query: impl Into<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, ene_store::ExportedMessage)>, String> {
        self.block_on_timeout(self.handle.search_sessions(query, limit, offset))
            .and_then(|r| r.map_err(|e| e.to_string()))
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|(session_id, message)| {
                        (session_id, exported_message_from_public(message))
                    })
                    .collect()
            })
    }

    /// Archive or unarchive a session, returning whether the archived
    /// flag actually changed (#176).
    pub fn archive_session_blocking(
        &self,
        session_id: impl Into<String>,
        archived: bool,
    ) -> Result<bool, String> {
        self.block_on_timeout(self.handle.archive_session(session_id, archived))
            .and_then(|r| r.map_err(|e| e.to_string()))
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
        self.block_on_timeout(self.handle.diagnostics().get_snapshot())
            .and_then(|r| r.map_err(|e| e.to_string()))
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
        self.block_on_timeout(ene_ai::validate_api_key(base_url, api_key))
            .and_then(|r| r.map_err(|e| e.to_string()))
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
        self.block_on_timeout(async {
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
        })?
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
        self.block_on_timeout(async {
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
        })?
    }

    /// Execute a journal action using the correct store API.
    pub fn execute_journal_action(
        &self,
        id: i64,
        action: MemoryJournalAction,
    ) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
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
        })?
    }

    /// Applies a memory lifecycle action.
    #[expect(dead_code, reason = "retained for transitional callers")]
    pub fn update_memory_status(
        &self,
        id: i64,
        status: ene_store::MemoryStatus,
    ) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(memory.set_memory_status(id, status))
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    /// Pins a memory row in the typed store.
    #[expect(dead_code, reason = "replaced by execute_journal_action")]
    pub fn pin_memory(&self, id: i64) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(memory.pin_typed_memory(id, true))
            .and_then(|r| r.map_err(|e| e.to_string()))
    }

    // ── Pending candidate approval (#174 / #223) ──

    /// List pending memory candidates awaiting user approval.
    pub fn fetch_pending_candidates(&self) -> Result<Vec<PendingCandidateSummary>, String> {
        self.block_on_timeout(self.handle.list_pending_candidates())
            .and_then(std::convert::identity)
    }

    /// Approve a pending memory candidate, persisting it as a typed memory.
    pub fn approve_candidate(&self, id: i64) -> Result<(), String> {
        self.block_on_timeout(self.handle.approve_candidate(id))
            .and_then(std::convert::identity)
    }

    /// Reject a pending memory candidate.
    pub fn reject_candidate(&self, id: i64) -> Result<(), String> {
        self.block_on_timeout(self.handle.reject_candidate(id))
            .and_then(std::convert::identity)
    }

    // ── Commitment management (#223) ──

    /// Mark a commitment as done.
    pub fn complete_commitment(&self, id: i64) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            let store = memory
                .store()
                .ok_or_else(|| "Memory store is not available".to_string())?;
            store
                .complete_commitment(id)
                .await
                .map_err(|e| e.to_string())
        })?
    }

    /// Mark a commitment as cancelled.
    pub fn cancel_commitment(&self, id: i64) -> Result<bool, String> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            let store = memory
                .store()
                .ok_or_else(|| "Memory store is not available".to_string())?;
            store.cancel_commitment(id).await.map_err(|e| e.to_string())
        })?
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
    audio_tx: Option<AudioChunkSender>,
) {
    // Turn that owns the in-flight TTS audio. Tracked separately from
    // `active_turn` because the fire-and-forget TTS worker keeps emitting
    // chunks after `Terminal` clears `active_turn` (H1). Only cleared by
    // the `is_final` marker or a playback reset on broadcast lag (M2).
    #[cfg(feature = "voice")]
    let mut audio_turn: Option<TurnId> = None;

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
                            let layer = cue.motion_layer.unwrap_or(ene_config::MotionLayer::Full);
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
            Ok(EneEvent::AudioChunk {
                turn,
                origin: _,
                pcm,
                sample_rate,
                is_final,
            }) => {
                // Audio chunks are forwarded to the playback subsystem
                // regardless of the active turn so TTS audio for the
                // current utterance plays even after the text stream
                // has finished. Without the `voice` feature there is no
                // playback subsystem, so the chunk is dropped.
                #[cfg(feature = "voice")]
                {
                    // Track the turn that owns the in-flight TTS audio
                    // separately from `active_turn`: the fire-and-forget
                    // TTS worker keeps emitting chunks after `Terminal`
                    // clears `active_turn`, so filtering on `active_turn`
                    // would drop the tail of the utterance (H1).
                    if !route_audio_chunk(&mut audio_turn, &turn, is_final) {
                        continue;
                    }
                    if let Some(tx) = &audio_tx {
                        // `try_send` avoids blocking the async pump when the
                        // bounded playback channel is full (L10); a full
                        // buffer means the sink is behind and the dropped
                        // chunk is preferable to stalling all event routing.
                        // The natural TTS final marker never aborts playback.
                        if let Err(e) = tx.try_send(AudioChunkPayload {
                            pcm,
                            sample_rate,
                            is_final,
                            abort: false,
                        }) {
                            tracing::warn!(
                                component = "AiBridge",
                                error = %e,
                                "playback channel full; dropping audio chunk"
                            );
                        }
                    }
                }
                #[cfg(not(feature = "voice"))]
                {
                    let _audio_tx = &audio_tx;
                    let _turn = turn;
                    let _pcm = pcm;
                    let _sample_rate = sample_rate;
                    let _is_final = is_final;
                }
            }
            Ok(EneEvent::PendingCandidateAvailable { count }) => {
                let _ = event_tx.send(AppEvent::PendingCandidatesCount(count));
            }
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
                reason:
                    reason
                    @ (ene_runtime::TerminalReason::Done | ene_runtime::TerminalReason::Cancelled),
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                clear_active_turn(&active_turn, &turn);
                processing.store(false, Ordering::Relaxed);
                // Ensure the playback subsystem releases `tts_playing` even
                // if the TTS worker never emitted an `is_final` marker (M2).
                // A cancelled turn is a barge-in: `abort` stops the sink
                // immediately instead of draining the queued audio.
                #[cfg(feature = "voice")]
                if audio_turn.take().is_some()
                    && let Some(tx) = &audio_tx
                {
                    let abort = matches!(reason, ene_runtime::TerminalReason::Cancelled);
                    // Blocking send: dropping a synthetic final marker would
                    // leave `tts_playing` stuck (self-voice suppression never
                    // releases), so wait for capacity rather than discarding.
                    if let Err(e) = tx.send(AudioChunkPayload {
                        pcm: Vec::new(),
                        sample_rate: 0,
                        is_final: true,
                        abort,
                    }) {
                        tracing::warn!(
                            component = "AiBridge",
                            error = %e,
                            "playback channel closed; dropping synthetic final marker"
                        );
                    }
                }
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
                #[cfg(feature = "voice")]
                if audio_turn.take().is_some()
                    && let Some(tx) = &audio_tx
                {
                    // Blocking send so a dropped marker cannot wedge
                    // `tts_playing` (see the Done/Cancelled arm).
                    if let Err(e) = tx.send(AudioChunkPayload {
                        pcm: Vec::new(),
                        sample_rate: 0,
                        is_final: true,
                        abort: false,
                    }) {
                        tracing::warn!(
                            component = "AiBridge",
                            error = %e,
                            "playback channel closed; dropping synthetic final marker"
                        );
                    }
                }
                let user_message = crate::runtime_error::user_message_from_turn(&message);
                let _ = event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(user_message)));
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    "[Ene] Dropped {n} events (broadcast lag); cancelling active turn if any"
                );
                // Unlock input, but keep active_turn and cancel so TurnGate
                // is freed even if Terminal was among the dropped events.
                processing.store(false, Ordering::Relaxed);
                // The `is_final` marker may have been among the dropped
                // events; send a synthetic final so `tts_playing` does not
                // stay stuck (M2). Use a blocking send so the marker itself
                // is never dropped (which would re-introduce the stuck flag).
                #[cfg(feature = "voice")]
                if audio_turn.take().is_some()
                    && let Some(tx) = &audio_tx
                    && let Err(e) = tx.send(AudioChunkPayload {
                        pcm: Vec::new(),
                        sample_rate: 0,
                        is_final: true,
                        abort: false,
                    })
                {
                    tracing::warn!(
                        component = "AiBridge",
                        error = %e,
                        "playback channel closed; dropping synthetic final marker"
                    );
                }
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
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::error!(
                    component = "AiBridge",
                    "Actor broadcast channel closed; runtime disconnected"
                );
                processing.store(false, Ordering::Relaxed);
                let shutdown_handle = handle.clone();
                tokio::spawn(async move {
                    let _ = shutdown_handle
                        .shutdown(std::time::Duration::from_secs(2))
                        .await;
                });
                let _ = event_tx.send(AppEvent::RuntimeDisconnected);
                break;
            }
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

/// Decide whether an incoming `AudioChunk` should be forwarded to the
/// playback subsystem, updating the tracked `audio_turn` in place (H1).
///
/// The fire-and-forget TTS worker keeps emitting chunks after `Terminal`
/// clears `active_turn`, so the audio path tracks its own turn ownership
/// instead of filtering on `active_turn`. Returns `true` when the chunk
/// should be forwarded.
///
/// - A non-final chunk claims `audio_turn` on first sight and is dropped
///   if it belongs to a different turn than the one currently claimed.
/// - A final marker is accepted only for the claimed turn (or when no
///   turn is claimed, e.g. an empty utterance) and clears `audio_turn`.
#[cfg(feature = "voice")]
fn route_audio_chunk(audio_turn: &mut Option<TurnId>, turn: &TurnId, is_final: bool) -> bool {
    if is_final {
        if audio_turn.as_ref().is_some_and(|t| t != turn) {
            return false;
        }
        *audio_turn = None;
        return true;
    }
    match audio_turn.as_ref() {
        None => {
            *audio_turn = Some(turn.clone());
            true
        }
        Some(current) if current != turn => false,
        Some(_) => true,
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

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_first_chunk_claims_turn() {
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        assert_eq!(audio_turn, Some(turn));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_same_turn_continues() {
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        route_audio_chunk(&mut audio_turn, &turn, false);
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_different_turn_dropped() {
        let mut audio_turn: Option<TurnId> = None;
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        route_audio_chunk(&mut audio_turn, &turn_a, false);
        assert!(!route_audio_chunk(&mut audio_turn, &turn_b, false));
        // Still claims turn_a.
        assert_eq!(audio_turn, Some(turn_a));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_final_clears_turn() {
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        route_audio_chunk(&mut audio_turn, &turn, false);
        assert!(route_audio_chunk(&mut audio_turn, &turn, true));
        assert_eq!(audio_turn, None);
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_final_wrong_turn_dropped() {
        let mut audio_turn: Option<TurnId> = None;
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        route_audio_chunk(&mut audio_turn, &turn_a, false);
        assert!(!route_audio_chunk(&mut audio_turn, &turn_b, true));
        assert_eq!(audio_turn, Some(turn_a));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_final_no_claim_accepted() {
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        assert!(route_audio_chunk(&mut audio_turn, &turn, true));
        assert_eq!(audio_turn, None);
    }

    #[cfg(feature = "voice")]
    #[test]
    fn route_audio_tail_after_terminal_not_dropped() {
        // H1 regression: after Terminal clears active_turn, the TTS
        // worker still emits chunks. The audio_turn tracker keeps them
        // flowing until is_final arrives.
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        // First chunk claims the turn.
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        // Simulate Terminal clearing active_turn (not audio_turn).
        // Subsequent chunks for the same turn must still be accepted.
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        // Final marker clears audio_turn.
        assert!(route_audio_chunk(&mut audio_turn, &turn, true));
        assert_eq!(audio_turn, None);
    }
}
