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
    AudioChunk, AudioStreamReceiver, EneEvent, EneEventReceiver, EneHandle, EneRuntimeError,
    LifecycleEvent, LifecycleReceiver, PermissionDecision, RunError, TurnId, UserInputResponse,
    open_with_config,
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

/// Payload returned by [`AiBridge::refresh_memory_ledger`].
pub struct MemoryLedgerPayload {
    pub memories: Vec<ene_store::MemoryItem>,
    pub commitments: Vec<ene_store::Commitment>,
}

/// Owns the actor handle. The runtime can also send user input
/// back through [`AiBridge::run`] and [`AiBridge::cancel`].
pub struct AiBridge {
    handle: EneHandle,
    runtime: tokio::runtime::Handle,
    /// Set on run, cleared on Terminal.
    processing: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<TurnId>>>,
    proactive_observe: ProactiveObserveControl,
}

/// Maximum time a `*_blocking` call may hold the winit main thread
/// before it is cancelled and an error is returned to the UI.
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors surfaced by [`AiBridge`]'s `*_blocking` wrapper methods to the
/// winit/egui UI layer.
///
/// This is the desktop app's own thin boundary type, not a second
/// competing definition of `ene_runtime::PublicApiError` — it wraps that
/// type (plus [`ene_runtime::EneRuntimeError`], already `thiserror`-typed
/// at its own crate boundary) and adds exactly one desktop-local concern
/// that doesn't belong in `ene-runtime`: a blocking call that exceeded
/// [`BLOCKING_TIMEOUT`] and was cancelled before the actor replied.
///
/// A dead actor reaches this type solely as
/// [`ene_runtime::PublicApiError::ActorDead`] (via the [`AiBridgeError::Api`]
/// variant), so there is no separate actor-dead error to fold in here.
#[derive(Debug, thiserror::Error)]
pub enum AiBridgeError {
    /// The blocking call exceeded [`BLOCKING_TIMEOUT`] and was cancelled.
    #[error("operation timed out after {0}s")]
    Timeout(u64),
    /// Host-internal wiring that still reports [`EneRuntimeError`] failed:
    /// runtime bootstrap ([`AiBridge::try_new`]) and the methods that also
    /// surface actor-side failures beyond a dead channel (`search_tools` /
    /// `call_tool`, which can additionally report `EneRuntimeError::Busy`,
    /// and `compress_context` / `set_character`).
    #[error(transparent)]
    Runtime(#[from] EneRuntimeError),
    /// One of the API v1 contract methods on [`EneHandle`] — or an
    /// actor-control / diagnostics method that reports a dead actor —
    /// returned a stable [`ene_runtime::PublicApiError`] category.
    #[error(transparent)]
    Api(#[from] ene_runtime::PublicApiError),
    /// An AI provider call made directly from the bridge (outside the
    /// actor) failed.
    #[error(transparent)]
    Ai(#[from] ene_ai::AiError),
    /// A connector framework operation failed.
    #[error(transparent)]
    Connector(#[from] ene_connector::ConnectorError),
    /// A connector operation through the runtime handle failed.
    #[error(transparent)]
    ConnectorHandle(#[from] ene_runtime::ConnectorHandleError),
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
        let lifecycle_receiver = handle.subscribe_lifecycle();
        // Claim the audio channel up front regardless of the `voice` feature
        // or whether a playback sender was supplied: the audio channel is
        // bounded and single-consumer, so this bridge must always be
        // the one consumer that drains it, or a sustained TTS stream could
        // eventually stall waiting to deliver a final marker (see
        // `send_audio_chunk` in ene-runtime).
        let audio_stream = handle.take_audio_stream();
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
            lifecycle_receiver,
            audio_stream,
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
    /// deadlocked: the future is cancelled after the timeout and a typed
    /// [`AiBridgeError::Timeout`] is returned to the caller.
    fn block_on_timeout<F, T>(&self, fut: F) -> Result<T, AiBridgeError>
    where
        F: std::future::Future<Output = T>,
    {
        self.runtime
            .block_on(tokio::time::timeout(BLOCKING_TIMEOUT, fut))
            .map_err(|_| AiBridgeError::Timeout(BLOCKING_TIMEOUT.as_secs()))
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

    pub fn cancel(&self) {
        let turn = self.active_turn.lock().ok().and_then(|g| g.clone());
        let Some(turn) = turn else {
            return;
        };
        if let Err(e) = self.handle.cancel(&turn) {
            tracing::warn!("[Ene] Failed to send Cancel command: {e}");
        }
    }

    /// Report a detected system-audio beat pulse to the runtime for
    /// broadcast on the chat bus. Called from the beat-sync capture thread.
    #[cfg(feature = "voice")]
    pub fn report_beat_pulse(&self, bpm: f32, intensity: f32) {
        if let Err(e) = self.handle.report_beat_pulse(bpm, intensity) {
            tracing::warn!(
                component = "BeatSync",
                error = %e,
                "failed to report beat pulse"
            );
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
        rag: &ene_rag::ToolRagConfig,
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
    ) -> Result<(), AiBridgeError> {
        Ok(self.handle.decide_permission(request_id, decision)?)
    }

    /// List the standing session-wide permission grants. Blocks the
    /// calling thread while the actor answers; intended for the
    /// permission-center settings page.
    pub fn list_permissions_blocking(
        &self,
    ) -> Result<Vec<ene_runtime::PermissionScope>, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.list_permissions())??)
    }

    /// Revoke a single standing permission grant by id.
    ///
    /// Returns whether a grant was actually removed.
    pub fn revoke_permission_blocking(&self, id: u64) -> Result<bool, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.revoke_permission(id))??)
    }

    /// Revoke every standing permission grant, returning the number
    /// removed.
    pub fn reset_all_permissions_blocking(&self) -> Result<usize, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.reset_all_permissions())??)
    }

    /// Lists the cached connector summaries (I/O-free).
    pub fn list_connectors_blocking(&self) -> Vec<ene_connector::ConnectorSummary> {
        self.handle.connectors().list()
    }

    /// Returns the cached status snapshot of one connector (I/O-free).
    pub fn connector_status_blocking(
        &self,
        id: &ene_connector::ConnectorId,
    ) -> Result<ene_connector::ConnectorStatus, AiBridgeError> {
        self.handle
            .connectors()
            .status(id)
            .map_err(AiBridgeError::from)
    }

    /// Lists the standing per-action grants of one connector (I/O-free).
    pub fn connector_permissions_blocking(
        &self,
        id: &ene_connector::ConnectorId,
    ) -> Result<Vec<ene_connector::PermissionGrant>, AiBridgeError> {
        self.handle
            .connectors()
            .permissions(id)
            .map_err(AiBridgeError::from)
    }

    /// Runs a connector connectivity check. Blocks the calling thread while
    /// the actor answers.
    pub fn check_connector_blocking(
        &self,
        id: &ene_connector::ConnectorId,
    ) -> Result<ene_connector::HealthStatus, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.connectors().check(id))??)
    }

    /// Undo the most recent reversible tool operation. Blocks the
    /// calling thread while the actor answers.
    pub fn undo_blocking(&self) -> Result<ene_runtime::UndoReport, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.undo())??)
    }

    /// Build an STT provider through the runtime's plugin host. Blocks the
    /// calling thread while the actor answers; used by the microphone
    /// capture path, which runs on the UI thread.
    #[cfg(feature = "voice")]
    pub fn create_stt_provider_blocking(
        &self,
        kind: &str,
    ) -> Result<Arc<dyn ene_ai::SttProvider>, AiBridgeError> {
        Ok(Arc::from(self.block_on_timeout(
            self.handle.create_stt_provider(kind),
        )??))
    }

    /// Build a VAD engine through the runtime's plugin host. Blocks the
    /// calling thread while the actor answers; used by the microphone
    /// capture path, which runs on the UI thread.
    #[cfg(feature = "voice")]
    pub fn create_vad_engine_blocking(
        &self,
        kind: &str,
    ) -> Result<Box<dyn ene_ai::VadEngine>, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.create_vad_engine(kind))??)
    }

    /// Open the session with the greeting at `index` (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`). Returns the applied greeting text.
    pub fn set_greeting_blocking(&self, index: u32) -> Result<String, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.set_greeting(index))??)
    }

    /// The currently loaded character card, if any. Mailbox-free.
    pub fn character_card(&self) -> Option<std::sync::Arc<ene_card::CharacterCardV3>> {
        self.handle.character_card()
    }

    /// List stored session metadata. Returns the API v1
    /// [`ene_runtime::PublicSessionMeta`] DTO directly — the desktop UI
    /// renders it as its canonical session type, so there is no reverse
    /// conversion to `ene_store::SessionMeta`.
    pub fn list_sessions_blocking(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Vec<ene_runtime::PublicSessionMeta>, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.sessions().list(include_archived, limit))??)
    }

    /// Export a session as a pretty-printed JSON string.
    pub fn export_session_blocking(
        &self,
        session_id: impl Into<String>,
    ) -> Result<String, AiBridgeError> {
        let session_id = session_id.into();
        Ok(self.block_on_timeout(self.handle.sessions().export(&session_id))??)
    }

    /// Import a session from a JSON string, returning the imported
    /// session's row id.
    pub fn import_session_blocking(&self, json: impl Into<String>) -> Result<i64, AiBridgeError> {
        let json = json.into();
        Ok(self.block_on_timeout(self.handle.sessions().import(&json))??)
    }

    /// Search session messages, returning matching
    /// `(session_id, message)` pairs. Messages are the API v1
    /// [`ene_runtime::PublicExportedMessage`] DTO, rendered directly by
    /// the desktop UI — no reverse conversion to `ene_store::ExportedMessage`.
    pub fn search_sessions_blocking(
        &self,
        query: impl Into<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<(String, ene_runtime::PublicExportedMessage)>, AiBridgeError> {
        let query = query.into();
        Ok(self.block_on_timeout(self.handle.sessions().search(&query, limit, offset))??)
    }

    /// Archive or unarchive a session, returning whether the archived
    /// flag actually changed.
    pub fn archive_session_blocking(
        &self,
        session_id: impl Into<String>,
        archived: bool,
    ) -> Result<bool, AiBridgeError> {
        let session_id = session_id.into();
        Ok(self.block_on_timeout(self.handle.sessions().set_archived(&session_id, archived))??)
    }

    /// Forward a `UserInputResponse` for the request
    /// currently sitting in `ChatState::pending_user_input`.
    pub fn answer_user_input(
        &self,
        request_id: impl Into<ene_runtime::RequestId>,
        response: UserInputResponse,
    ) -> Result<(), AiBridgeError> {
        Ok(self.handle.submit_user_input(request_id, response)?)
    }

    /// Fetches a fresh actor snapshot on the runtime thread.
    pub fn get_snapshot_blocking(&self) -> Result<ene_runtime::EneStateSnapshot, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.diagnostics().get_snapshot())??)
    }

    /// Snapshot of cached provider health reports for the AI settings page.
    pub fn provider_health_reports(&self) -> Vec<ene_ai::ProviderHealthReport> {
        self.handle.diagnostics().provider_health_reports()
    }

    /// Snapshot of recent provider fallback events for the AI settings page.
    pub fn provider_fallback_history(&self) -> Vec<ene_ai::FallbackRecord> {
        self.handle.diagnostics().provider_fallback_history()
    }

    /// Run [`ene_ai::validate_api_key`] on the bridge runtime.
    pub fn validate_api_key_blocking(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<(), AiBridgeError> {
        Ok(self.block_on_timeout(ene_ai::validate_api_key(base_url, api_key))??)
    }

    /// Snapshot the provider kinds registered by the plugin host.
    pub fn provider_catalog_blocking(&self) -> Result<ene_runtime::ProviderCatalog, AiBridgeError> {
        self.block_on_timeout(self.handle.provider_catalog())
    }

    /// Run [`ene_ai::fetch_model_ids`] on the bridge runtime.
    pub fn list_models_blocking(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<Vec<String>, AiBridgeError> {
        Ok(self.block_on_timeout(ene_ai::fetch_model_ids(base_url, api_key))??)
    }

    /// Refresh memory journal payload (typed memories + affect + commitments).
    pub fn refresh_memory_journal(
        &self,
        limit: usize,
        include_user_deleted: bool,
        include_archived: bool,
        include_superseded: bool,
    ) -> Result<MemoryJournalPayload, AiBridgeError> {
        // Mailbox-free state reads: the journal is refreshed from UI
        // actions, so it must never queue behind an in-flight turn.
        let character_id = self.handle.card_name();
        let user_id = self.handle.config().user_name.clone();
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            let options = ene_store::MemoryJournalListOptions {
                character_id: &character_id,
                user_id: Some(user_id.as_str()),
                include_archived,
                include_superseded,
                include_user_deleted,
                kind: None,
                limit,
                offset: 0,
            };
            let memories = memory.list_journal_memories(&options).await?;
            let affect = memory.show_affect_state(&character_id).await?;
            let commitments = memory
                .list_active_commitments(&character_id, Some(&user_id), limit)
                .await?;
            let (pending_writes, permanent_writes) =
                memory.count_pending_memory_writes(&character_id).await?;
            Ok(MemoryJournalPayload {
                memories,
                affect,
                commitments,
                pending_writes,
                permanent_writes,
            })
        })?
    }

    /// Refresh the memory ledger payload (typed memories in every status +
    /// commitments in every lifecycle state).
    pub fn refresh_memory_ledger(
        &self,
        limit: usize,
    ) -> Result<MemoryLedgerPayload, AiBridgeError> {
        // Mailbox-free state reads; see `refresh_memory_journal`.
        let character_id = self.handle.card_name();
        let user_id = self.handle.config().user_name.clone();
        let ledger = self.handle.memory_ledger();
        self.block_on_timeout(async {
            let options = ene_store::MemoryJournalListOptions {
                character_id: &character_id,
                user_id: Some(user_id.as_str()),
                include_archived: true,
                include_superseded: true,
                include_user_deleted: true,
                kind: None,
                limit,
                offset: 0,
            };
            let memories = ledger.list_memories(&options).await?;
            let commitments = ledger
                .list_commitments(Some(user_id.as_str()), None, limit)
                .await?;
            Ok(MemoryLedgerPayload {
                memories,
                commitments,
            })
        })?
    }

    /// Edit a persisted typed memory in place through the ledger handle.
    pub fn edit_memory_blocking(
        &self,
        id: i64,
        edit: ene_store::MemoryEdit,
    ) -> Result<(), AiBridgeError> {
        Ok(
            self.block_on_timeout(self.handle.memory_ledger().edit_memory(
                id,
                edit,
                self.handle.active_turn(),
            ))??,
        )
    }

    /// Adjust a typed memory's salience (importance / Preference weight).
    pub fn set_memory_salience_blocking(
        &self,
        id: i64,
        salience: f32,
    ) -> Result<(), AiBridgeError> {
        Ok(
            self.block_on_timeout(self.handle.memory_ledger().set_memory_salience(
                id,
                salience,
                self.handle.active_turn(),
            ))??,
        )
    }

    /// Run explainable recall search for the journal debug mode.
    pub fn search_memory_journal_recall(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryJournalRecallRow>, AiBridgeError> {
        // Mailbox-free state reads; see `refresh_memory_journal`.
        let character_id = self.handle.card_name();
        let user_id = self.handle.config().user_name.clone();
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            let recalled = memory
                .search_typed_memories_explained(&character_id, Some(&user_id), query, limit)
                .await?;
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
    ) -> Result<bool, AiBridgeError> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            match action {
                MemoryJournalAction::Pin => memory.pin_typed_memory(id, true).await,
                MemoryJournalAction::Unpin => memory.pin_typed_memory(id, false).await,
                MemoryJournalAction::Archive => {
                    memory
                        .set_memory_status(id, ene_store::MemoryStatus::Archived)
                        .await
                }
                MemoryJournalAction::Forget => memory.user_forget_typed_memory(id).await,
                MemoryJournalAction::Dispute => {
                    memory
                        .set_memory_status(id, ene_store::MemoryStatus::Disputed)
                        .await
                }
                MemoryJournalAction::Restore => memory.user_restore_typed_memory(id).await,
            }
            .map_err(AiBridgeError::from)
        })?
    }

    // ── Pending candidate approval ──

    /// List pending memory candidates awaiting user approval.
    pub fn fetch_pending_candidates(&self) -> Result<Vec<PendingCandidateSummary>, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.candidates().list_pending())??)
    }

    /// Approve a pending memory candidate, persisting it as a typed memory.
    pub fn approve_candidate(&self, id: i64) -> Result<(), AiBridgeError> {
        Ok(self.block_on_timeout(
            self.handle
                .candidates()
                .approve(id, self.handle.active_turn()),
        )??)
    }

    /// Reject a pending memory candidate.
    pub fn reject_candidate(&self, id: i64) -> Result<(), AiBridgeError> {
        Ok(self.block_on_timeout(
            self.handle
                .candidates()
                .reject(id, self.handle.active_turn()),
        )??)
    }

    /// Edit a still-pending candidate's user-editable fields.
    pub fn edit_candidate(
        &self,
        id: i64,
        edit: ene_runtime::PendingCandidateEdit,
    ) -> Result<(), AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.candidates().edit(
            id,
            edit,
            self.handle.active_turn(),
        ))??)
    }

    /// List approved / rejected candidates, newest first.
    pub fn fetch_candidate_history(&self) -> Result<Vec<PendingCandidateSummary>, AiBridgeError> {
        Ok(self.block_on_timeout(self.handle.candidates().history(50))??)
    }

    // ── Commitment management ──

    /// Mark a commitment as done.
    pub fn complete_commitment(&self, id: i64) -> Result<bool, AiBridgeError> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            memory
                .complete_commitment(id)
                .await
                .map_err(AiBridgeError::from)
        })?
    }

    /// Mark a commitment as cancelled.
    pub fn cancel_commitment(&self, id: i64) -> Result<bool, AiBridgeError> {
        let memory = self.handle.diagnostics().memory().clone();
        self.block_on_timeout(async {
            memory
                .cancel_commitment(id)
                .await
                .map_err(AiBridgeError::from)
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

/// Awaits the next audio chunk, or never resolves when no audio stream was
/// claimed (`take_audio_stream` returned `None`) or it has since closed.
///
/// Used as a `tokio::select!` branch in [`pump_events`] so the absence of an
/// audio stream degrades to "this branch never fires" instead of a busy
/// loop. Once the underlying `mpsc::Receiver` reports closed (`None`), the
/// `Option` is cleared so subsequent polls also degrade to pending rather
/// than immediately resolving with `None` again and again.
async fn recv_audio_chunk(stream: &mut Option<AudioStreamReceiver>) -> Option<AudioChunk> {
    match stream {
        Some(rx) => {
            if let Some(chunk) = rx.recv().await {
                Some(chunk)
            } else {
                *stream = None;
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

/// Forwards one audio chunk to the desktop playback subsystem, applying
/// the same turn-ownership gate as the chat path.
///
/// Without the `voice` feature there is no playback subsystem, so the
/// chunk is dropped.
fn forward_audio_chunk(
    chunk: AudioChunk,
    audio_turn: &mut Option<TurnId>,
    audio_tx: Option<&AudioChunkSender>,
) {
    #[cfg(feature = "voice")]
    {
        let AudioChunk {
            turn,
            origin: _,
            pcm,
            sample_rate,
            is_final,
            cues,
        } = chunk;
        // Track the turn that owns the in-flight TTS audio separately from
        // `active_turn`: the fire-and-forget TTS worker keeps emitting
        // chunks after `Terminal` clears `active_turn`, so filtering on
        // `active_turn` would drop the tail of the utterance.
        if !route_audio_chunk(audio_turn, &turn, is_final) {
            return;
        }
        if let Some(tx) = audio_tx {
            // `try_send` avoids blocking the async pump when the bounded
            // playback channel is full; a full buffer means the sink
            // is behind and the dropped chunk is preferable to stalling all
            // event routing. The natural TTS final marker never aborts
            // playback.
            if let Err(e) = tx.try_send(AudioChunkPayload {
                pcm,
                sample_rate,
                is_final,
                abort: false,
                cues,
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
        let _ = audio_turn;
        let _ = audio_tx;
        let _ = chunk;
    }
}

async fn pump_events(
    mut receiver: EneEventReceiver,
    mut lifecycle_receiver: LifecycleReceiver,
    mut audio_stream: Option<AudioStreamReceiver>,
    event_tx: AppEventSender,
    handle: EneHandle,
    processing: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<TurnId>>>,
    audio_tx: Option<AudioChunkSender>,
) {
    // TTS audio ownership, tracked separately from `active_turn` because
    // the fire-and-forget TTS worker keeps emitting chunks after
    // `Terminal` clears `active_turn`. Only cleared by the `is_final`
    // marker or a playback reset on broadcast lag.
    let mut audio_turn: Option<TurnId> = None;

    loop {
        tokio::select! {
        chat_event = receiver.recv() => match chat_event {
            Ok(EneEvent::TextDelta {
                turn,
                origin: _,
                delta,
            }) => {
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::TextDelta(delta))));
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
                            let layer = cue.motion_layer.unwrap_or(ene_card::MotionLayer::Full);
                            let priority = cue_source_to_u8(source);
                            drop(event_tx.send(AppEvent::MotionCue {
                                name: cue.name,
                                layer,
                                priority,
                                duration: 0.0,
                            }));
                        }
                        ene_mind::PerfKind::Expression => {
                            let weight = cue.weight.unwrap_or(ene_mind::DEFAULT_EXPRESSION_WEIGHT);
                            let hold_secs =
                                cue.hold_secs.unwrap_or(ene_mind::DEFAULT_EXPRESSION_HOLD_SECS);
                            // Turn-end cues apply immediately; mid-utterance
                            // TTS-synced cues arrive via the audio chunk path
                            // with a scheduled `target_time`.
                            drop(event_tx.send(AppEvent::ExpressionCue {
                                name: cue.name,
                                weight,
                                hold_secs,
                                target_time: 0.0,
                            }));
                        }
                        ene_mind::PerfKind::Cancel => {
                            drop(event_tx.send(AppEvent::CancelCue { scope: cue.name }));
                        }
                        ene_mind::PerfKind::LookAt => {
                            drop(event_tx.send(AppEvent::LookAtCue {
                                target: cue.name.clone(),
                            }));
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
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallStart {
                    name,
                    arguments,
                })));
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
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::ToolCallResult {
                    name,
                    result,
                })));
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
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                })));
            }
            Ok(EneEvent::BrokerApprovalRequired {
                request_id,
                plugin,
                category,
                target,
                description,
            }) => {
                // Broker approvals are not tied to a turn; reuse the
                // permission-prompt UI with the plugin/category in the
                // action label.
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::PermissionRequired {
                    request_id,
                    action: format!("{plugin}:{category}"),
                    target,
                    description,
                })));
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
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::UserInputRequired {
                    request_id,
                    prompt,
                })));
            }
            Ok(EneEvent::ContextCompressed { .. }) => {}
            Ok(EneEvent::TurnStarted { turn, origin: _ }) => {
                if let Ok(mut guard) = active_turn.lock() {
                    *guard = Some(turn);
                }
                processing.store(true, Ordering::Relaxed);
            }
            Ok(EneEvent::BeatPulse { bpm, intensity }) => {
                // Turn-independent: no `turn_matches` gate.
                drop(event_tx.send(AppEvent::BeatPulse { bpm, intensity }));
            }
            Ok(EneEvent::Terminal {
                turn,
                origin: _,
                reason:
                    reason
                    @ (ene_runtime::TerminalReason::Done
                    | ene_runtime::TerminalReason::Cancelled
                    | ene_runtime::TerminalReason::Declined),
            }) => {
                #[cfg(not(feature = "voice"))]
                let _ = reason;
                if !turn_matches(&active_turn, &turn) {
                    continue;
                }
                clear_active_turn(&active_turn, &turn);
                processing.store(false, Ordering::Relaxed);
                // Ensure the playback subsystem releases `tts_playing` even
                // if the TTS worker never emitted an `is_final` marker.
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
                        cues: Vec::new(),
                    }) {
                        tracing::warn!(
                            component = "AiBridge",
                            error = %e,
                            "playback channel closed; dropping synthetic final marker"
                        );
                    }
                }
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished)));
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
                        cues: Vec::new(),
                    }) {
                        tracing::warn!(
                            component = "AiBridge",
                            error = %e,
                            "playback channel closed; dropping synthetic final marker"
                        );
                    }
                }
                let user_message = crate::runtime_error::user_message_from_turn(&message);
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::Error(user_message))));
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
                // stay stuck. Use a blocking send so the marker itself
                // is never dropped (which would re-introduce the stuck flag).
                #[cfg(feature = "voice")]
                if audio_turn.take().is_some()
                    && let Some(tx) = &audio_tx
                    && let Err(e) = tx.send(AudioChunkPayload {
                        pcm: Vec::new(),
                        sample_rate: 0,
                        is_final: true,
                        abort: false,
                        cues: Vec::new(),
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
                drop(event_tx.send(AppEvent::Ai(AiStreamUpdate::Finished)));
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::error!(
                    component = "AiBridge",
                    "Actor broadcast channel closed; runtime disconnected"
                );
                processing.store(false, Ordering::Relaxed);
                let shutdown_handle = handle.clone();
                tokio::spawn(async move {
                    drop(shutdown_handle
                        .shutdown(std::time::Duration::from_secs(2))
                        .await);
                });
                drop(event_tx.send(AppEvent::RuntimeDisconnected));
                break;
            }
        },
        // Lifecycle bus: turn-independent, low-frequency. Only
        // `PendingCandidateAvailable` currently drives UI state here;
        // `StatusChanged` / `ToolBackgroundCompleted` / `CandidateChanged`
        // are observed elsewhere (diagnostics / background tool UI) or the
        // journal refetches after its own mutations anyway.
        lifecycle_event = lifecycle_receiver.recv() => match lifecycle_event {
            Ok(LifecycleEvent::PendingCandidateAvailable { count }) => {
                drop(event_tx.send(AppEvent::PendingCandidatesCount(count)));
            }
            Ok(
                LifecycleEvent::StatusChanged { .. }
                | LifecycleEvent::ToolBackgroundCompleted { .. }
                | LifecycleEvent::CandidateChanged { .. }
                | LifecycleEvent::ConnectorChanged { .. }
                | LifecycleEvent::MemoryLedgerChanged { .. },
            ) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("[Ene] Dropped {n} lifecycle events (broadcast lag)");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                // The actor is gone; the chat-bus `Closed` arm above already
                // handles the shutdown/reconnect flow, so this is a no-op.
            }
        },
        // Audio channel: bounded mpsc, single-consumer. Chunks are
        // forwarded regardless of `active_turn` — see
        // `forward_audio_chunk`.
        chunk = recv_audio_chunk(&mut audio_stream) => {
            if let Some(chunk) = chunk {
                forward_audio_chunk(chunk, &mut audio_turn, audio_tx.as_ref());
            }
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
/// playback subsystem, updating the tracked `audio_turn` in place.
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

    #[test]
    fn processing_flag_round_trip() {
        let processing = Arc::new(AtomicBool::new(false));
        processing.store(true, Ordering::Relaxed);
        assert!(processing.load(Ordering::Relaxed));
        processing.store(false, Ordering::Relaxed);
        assert!(!processing.load(Ordering::Relaxed));
    }

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
        // After `Terminal` clears `active_turn`, the TTS worker still
        // emits chunks. The audio_turn tracker keeps them flowing
        // until is_final arrives.
        let mut audio_turn: Option<TurnId> = None;
        let turn = TurnId::new();
        // First chunk claims the turn; then `Terminal` clears
        // `active_turn` (not `audio_turn`), so the trailing chunks for the
        // same turn must still be accepted until the final marker.
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        assert!(route_audio_chunk(&mut audio_turn, &turn, false));
        assert!(route_audio_chunk(&mut audio_turn, &turn, true));
        assert_eq!(audio_turn, None);
    }
}
