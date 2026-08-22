use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ene_body::{
    BodyCatalog, BodyError, BodySettings, EmotionCue, PerformanceBus, Stage, VoiceRuntime,
    VoiceSettings,
};
use ene_companion::{
    CharacterSettings, CompanionRuntime, CompanionStore, MindSettings, NewSoul, QueryEmbed,
    SlotQueryEmbed, register_memory_tools,
};
use ene_fiber::Supervisor;
use ene_kernel::{
    AiSettings, ConversationModel, CoreSettings, HarnessSettings, LaneHandle, LaneMindSettings,
    LaneOptions, LoopHooks, PluginSettings, SpeechPresenter, SurfaceRouter, TaskBinding,
    TurnFinalizer, TurnPrefetch, format_recovery_note,
};
use ene_plane::{
    ApprovalMode, ApprovalPlane, ApprovalSettings, AuditLog, PendingPopup, PolicyFile, PopupSink,
    Vault,
};
use ene_registry::ToolRegistry;
use ene_session::{
    BodyId, EventKind, EventPayload, NewEvent, RecoveryReport, SessionEndReason, SessionId,
    SessionStore, SoulId, StoreSettings, Transaction, v1,
};
use ene_work::{
    CompanionReport, DelegationHost, WorkError, WorkStore, WorkSurfaceRouter, register_work_tools,
    workspace_root,
};
use fs2::FileExt;
use thiserror::Error;
use tracing::info;

/// Options for booting the core daemon (W0: store + recovery, no HTTP).
#[derive(Debug, Clone)]
pub struct BootOptions {
    /// Data directory that holds `sessions.db` and the lock file.
    pub data_dir: PathBuf,
    /// `SQLite` `synchronous` pragma override (`NORMAL` or `FULL`).
    /// `None` uses `store.sessions.synchronous` from the config pipeline.
    pub synchronous: Option<String>,
}

impl BootOptions {
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            synchronous: None,
        }
    }
}

/// Boot / lock failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CoreError {
    #[error(transparent)]
    Session(#[from] ene_session::SessionError),
    #[error(transparent)]
    Kernel(#[from] ene_kernel::KernelError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Plane(#[from] ene_plane::PlaneError),
    #[error(transparent)]
    Audit(#[from] ene_plane::AuditError),
    #[error(transparent)]
    Vault(#[from] ene_plane::VaultError),
    #[error(transparent)]
    Companion(#[from] ene_companion::CompanionError),
    #[error(transparent)]
    Body(#[from] BodyError),
    #[error(transparent)]
    Work(#[from] WorkError),
    #[error("http: {0}")]
    Http(String),
    #[error("another ene-core instance holds the data-directory lock at {0}")]
    AlreadyRunning(String),
    #[error(transparent)]
    Config(#[from] ene_config::EneConfigError),
}

/// Running core: exclusive lock, session store, D-5 recovery reports, plugin supervisor.
pub struct CoreDaemon {
    data_dir: PathBuf,
    _lock: File,
    store: Arc<SessionStore>,
    recovery: Vec<RecoveryReport>,
    supervisor: Arc<Supervisor>,
    plane: Arc<ApprovalPlane>,
    vault: Vault,
    companions: Arc<CompanionStore>,
    companion: Arc<CompanionRuntime>,
    stage: Arc<Stage>,
    work: Arc<WorkStore>,
    host: Arc<DelegationHost>,
    job_reports: Vec<CompanionReport>,
    popup: Arc<PendingPopup>,
    settings: CoreSettings,
    ai: Arc<parking_lot::Mutex<ene_kernel::AiSettings>>,
    plugins: Arc<parking_lot::Mutex<PluginSettings>>,
    chat_secret: Arc<parking_lot::Mutex<String>>,
    classifier_secret: Arc<parking_lot::Mutex<String>>,
    embedding_secret: Arc<parking_lot::Mutex<String>>,
    proactive_secret: Arc<parking_lot::Mutex<String>>,
    tts_secret: Arc<parking_lot::Mutex<String>>,
    stt_secret: Arc<parking_lot::Mutex<String>>,
    approve_secret: Arc<parking_lot::Mutex<String>>,
    job_secret: Arc<parking_lot::Mutex<String>>,
    job_model: parking_lot::Mutex<Option<Arc<dyn ConversationModel>>>,
    speech: parking_lot::Mutex<Option<Arc<dyn SpeechPresenter>>>,
    finalizer: parking_lot::Mutex<Option<Arc<dyn ene_kernel::TurnFinalizer>>>,
    prefetch: parking_lot::Mutex<Option<Arc<dyn TurnPrefetch>>>,
    loop_hooks: LoopHooks,
    mind: parking_lot::Mutex<MindSettings>,
    harness: parking_lot::Mutex<HarnessSettings>,
    store_settings: parking_lot::Mutex<StoreSettings>,
    approval: parking_lot::Mutex<ApprovalSettings>,
    body: parking_lot::Mutex<BodySettings>,
    voice: parking_lot::Mutex<VoiceSettings>,
    characters: parking_lot::Mutex<CharacterSettings>,
    last_proactive: parking_lot::Mutex<HashMap<SessionId, Instant>>,
    observation: parking_lot::Mutex<ene_work::ObservationPipeline>,
    world_state: parking_lot::Mutex<ene_companion::WorldStateMemory>,
    memory_embed: Arc<SlotQueryEmbed>,
    idle_timeout_secs: parking_lot::Mutex<u64>,
}

impl CoreDaemon {
    /// Ensure the data dir, take the exclusive lock, open the store, recover interrupts.
    pub async fn boot(opts: BootOptions) -> Result<Self, CoreError> {
        std::fs::create_dir_all(&opts.data_dir)?;
        let loaded = load_boot_config(&opts.data_dir)?;
        let settings = loaded.core;
        let ai = loaded.ai;
        let plugins = loaded.plugins;
        let mind = loaded.mind;
        let lock = lock_data_dir(&opts.data_dir)?;
        let db_path = opts.data_dir.join("sessions.db");
        let sync = opts
            .synchronous
            .clone()
            .unwrap_or_else(|| loaded.store.sessions.synchronous.clone());
        let store = SessionStore::open(&db_path, &sync).await?;
        let recovery = store.recover_interrupted().await?;
        if !recovery.is_empty() {
            info!(
                sessions = recovery.len(),
                "detected interrupted work; closed without resume"
            );
        }
        let workspace = workspace_root(&opts.data_dir);
        std::fs::create_dir_all(&workspace)?;
        let registry = Arc::new(ToolRegistry::new());
        registry.set_workspace(workspace.clone());
        let audit = AuditLog::open(opts.data_dir.join("audit.db"))?;
        let popup = Arc::new(PendingPopup::new());
        let approval_settings = loaded.approval.clone();
        let plane = Arc::new(ApprovalPlane::new(
            approval_settings.clone(),
            audit,
            Arc::clone(&popup) as Arc<dyn PopupSink>,
            None,
        ));
        let policy_path = opts.data_dir.join(&approval_settings.policy_file);
        let policy = PolicyFile::load_json(&policy_path)?;
        plane.set_policy(policy);
        plane.set_policy_path(policy_path);
        registry.set_plane(Arc::clone(&plane));
        let vault = Vault::open_or_create_keyfile(
            opts.data_dir.join("vault.bin"),
            opts.data_dir.join("vault.key"),
        )?;
        let companions = Arc::new(CompanionStore::open(opts.data_dir.join("companions.db"))?);
        let memory_embed = Arc::new(SlotQueryEmbed::default());
        register_memory_tools(
            &registry,
            Arc::clone(&companions),
            Some(Arc::clone(&memory_embed) as Arc<dyn QueryEmbed>),
        );
        let work = Arc::new(WorkStore::open(opts.data_dir.join("companions.db"))?);
        let host = Arc::new(DelegationHost::new(
            Arc::clone(&work),
            opts.data_dir.clone(),
        ));
        register_work_tools(&registry, Arc::clone(&host), opts.data_dir.join("skills"));
        let job_reports = host.recover_interrupted()?;
        if !job_reports.is_empty() {
            info!(
                jobs = job_reports.len(),
                "detected interrupted tasks; closed without resume"
            );
        }
        let companion = Arc::new(CompanionRuntime::new(Arc::clone(&companions), mind.clone()));
        let bus = Arc::new(PerformanceBus::default());
        let voice_settings = loaded.voice.clone();
        let body_settings = loaded.body.clone();
        let stage = Arc::new(Stage::new(
            bus,
            VoiceRuntime::live(voice_settings.clone()),
            body_settings.clone(),
        ));
        let supervisor = Arc::new(Supervisor::new(workspace, registry));
        let loop_hooks = LoopHooks::new();
        supervisor.set_loop_hooks(loop_hooks.clone());
        #[cfg(test)]
        supervisor.set_prefer_in_process_builtins(true);
        seed_default_occupants(&companions, &stage)?;
        let chat_secret = load_named_secret(&vault, "ENE_AI__TASKS__CHAT__API_KEY", "ai.chat");
        let classifier_secret = load_named_secret(
            &vault,
            "ENE_AI__TASKS__CLASSIFIER__API_KEY",
            "ai.classifier",
        );
        let embedding_secret =
            load_named_secret(&vault, "ENE_AI__TASKS__EMBEDDING__API_KEY", "ai.embedding");
        let proactive_secret =
            load_named_secret(&vault, "ENE_AI__TASKS__PROACTIVE__API_KEY", "ai.proactive");
        let tts_secret = load_named_secret(&vault, "ENE_AI__TASKS__TTS__API_KEY", "ai.tts");
        let stt_secret = load_named_secret(&vault, "ENE_AI__TASKS__STT__API_KEY", "ai.stt");
        let approve_secret =
            load_named_secret(&vault, "ENE_AI__TASKS__APPROVE__API_KEY", "ai.approve");
        let job_secret = load_named_secret(&vault, "ENE_AI__TASKS__JOB__API_KEY", "ai.job");
        Ok(Self {
            data_dir: opts.data_dir,
            _lock: lock,
            store: Arc::new(store),
            recovery,
            supervisor,
            plane,
            vault,
            companions,
            companion,
            stage,
            work,
            host,
            job_reports,
            popup,
            settings,
            ai: Arc::new(parking_lot::Mutex::new(ai)),
            plugins: Arc::new(parking_lot::Mutex::new(plugins)),
            chat_secret: Arc::new(parking_lot::Mutex::new(chat_secret)),
            classifier_secret: Arc::new(parking_lot::Mutex::new(classifier_secret)),
            embedding_secret: Arc::new(parking_lot::Mutex::new(embedding_secret)),
            proactive_secret: Arc::new(parking_lot::Mutex::new(proactive_secret)),
            tts_secret: Arc::new(parking_lot::Mutex::new(tts_secret)),
            stt_secret: Arc::new(parking_lot::Mutex::new(stt_secret)),
            approve_secret: Arc::new(parking_lot::Mutex::new(approve_secret)),
            job_secret: Arc::new(parking_lot::Mutex::new(job_secret)),
            job_model: parking_lot::Mutex::new(None),
            speech: parking_lot::Mutex::new(None),
            finalizer: parking_lot::Mutex::new(None),
            prefetch: parking_lot::Mutex::new(None),
            loop_hooks,
            mind: parking_lot::Mutex::new(mind),
            harness: parking_lot::Mutex::new(loaded.harness),
            approval: parking_lot::Mutex::new(approval_settings),
            body: parking_lot::Mutex::new(body_settings),
            voice: parking_lot::Mutex::new(voice_settings),
            characters: parking_lot::Mutex::new(loaded.characters),
            last_proactive: parking_lot::Mutex::new(HashMap::new()),
            observation: parking_lot::Mutex::new(ene_work::ObservationPipeline::new()),
            world_state: parking_lot::Mutex::new(ene_companion::WorldStateMemory::default()),
            memory_embed,
            idle_timeout_secs: parking_lot::Mutex::new(loaded.store.sessions.idle_timeout_secs),
            store_settings: parking_lot::Mutex::new(loaded.store),
        })
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    #[must_use]
    pub fn character_home(&self) -> PathBuf {
        resolve_data_subdir(
            &self.data_dir,
            &self.characters.lock().home_dir,
            "characters",
        )
    }

    #[must_use]
    pub fn workspace_dir(&self) -> PathBuf {
        workspace_root(&self.data_dir)
    }

    #[must_use]
    pub fn settings(&self) -> &CoreSettings {
        &self.settings
    }

    #[must_use]
    pub fn ai(&self) -> Arc<parking_lot::Mutex<AiSettings>> {
        Arc::clone(&self.ai)
    }

    #[must_use]
    pub fn plugins(&self) -> Arc<parking_lot::Mutex<PluginSettings>> {
        Arc::clone(&self.plugins)
    }

    /// Vault value for `ai.tasks.<task>`, falling back to the chat key.
    #[must_use]
    pub fn secret_for(&self, task: &str) -> String {
        let named = match task {
            "classifier" => self.classifier_secret.lock().clone(),
            "embedding" => self.embedding_secret.lock().clone(),
            "proactive" => self.proactive_secret.lock().clone(),
            "tts" => self.tts_secret.lock().clone(),
            "stt" => self.stt_secret.lock().clone(),
            "approve" => self.approve_secret.lock().clone(),
            "job" => self.job_secret.lock().clone(),
            "chat" => self.chat_secret.lock().clone(),
            _ => String::new(),
        };
        if named.is_empty() && task != "chat" {
            self.chat_secret.lock().clone()
        } else {
            named
        }
    }

    #[must_use]
    pub fn task_key_set(&self, task: &str) -> bool {
        let named = match task {
            "classifier" => self.classifier_secret.lock().clone(),
            "embedding" => self.embedding_secret.lock().clone(),
            "proactive" => self.proactive_secret.lock().clone(),
            "tts" => self.tts_secret.lock().clone(),
            "stt" => self.stt_secret.lock().clone(),
            "approve" => self.approve_secret.lock().clone(),
            "job" => self.job_secret.lock().clone(),
            _ => self.chat_secret.lock().clone(),
        };
        !named.is_empty()
    }

    pub fn set_speech(&self, speech: Arc<dyn SpeechPresenter>) {
        *self.speech.lock() = Some(speech);
    }

    pub fn set_finalizer(&self, finalizer: Arc<dyn TurnFinalizer>) {
        *self.finalizer.lock() = Some(finalizer);
    }

    pub fn set_prefetch(&self, prefetch: Arc<dyn TurnPrefetch>) {
        *self.prefetch.lock() = Some(prefetch);
    }

    #[must_use]
    pub(crate) fn soul_skill_refs(&self, soul: SoulId) -> Vec<String> {
        self.companions()
            .get_soul(soul)
            .ok()
            .flatten()
            .map(|row| row.skill_refs)
            .unwrap_or_default()
    }

    pub fn set_job_model(&self, model: Arc<dyn ConversationModel>) {
        *self.job_model.lock() = Some(model);
    }

    #[must_use]
    pub fn job_model(&self) -> Option<Arc<dyn ConversationModel>> {
        self.job_model.lock().clone()
    }

    pub fn clear_turn_seams(&self) {
        *self.speech.lock() = None;
        *self.finalizer.lock() = None;
        *self.prefetch.lock() = None;
    }

    #[must_use]
    pub fn mind(&self) -> MindSettings {
        self.mind.lock().clone()
    }

    pub fn replace_mind(&self, mind: MindSettings) {
        *self.mind.lock() = mind.clone();
        self.companion.replace_settings(mind);
    }

    #[must_use]
    pub fn harness(&self) -> HarnessSettings {
        self.harness.lock().clone()
    }

    pub fn replace_harness(&self, harness: HarnessSettings) {
        *self.harness.lock() = harness;
    }

    #[must_use]
    pub fn store_settings(&self) -> StoreSettings {
        self.store_settings.lock().clone()
    }

    pub fn replace_store_settings(&self, settings: StoreSettings) {
        *self.idle_timeout_secs.lock() = settings.sessions.idle_timeout_secs;
        *self.store_settings.lock() = settings;
    }

    #[must_use]
    pub fn approval_settings(&self) -> ApprovalSettings {
        self.approval.lock().clone()
    }

    pub fn replace_approval(&self, settings: ApprovalSettings) {
        if let Err(err) = self.plane.set_mode(settings.mode) {
            tracing::warn!(error = %err, "failed to apply live approval mode");
        }
        let policy_path = self.data_dir.join(&settings.policy_file);
        match PolicyFile::load_json(&policy_path) {
            Ok(policy) => {
                self.plane.set_policy(policy);
                self.plane.set_policy_path(policy_path);
            }
            Err(err) => tracing::warn!(
                error = %err,
                path = %settings.policy_file,
                "failed to reload approval.policy_file"
            ),
        }
        *self.approval.lock() = settings;
    }

    #[must_use]
    pub fn body_settings(&self) -> BodySettings {
        self.body.lock().clone()
    }

    pub fn replace_body_settings(&self, settings: BodySettings) {
        self.stage.replace_body_settings(settings.clone());
        *self.body.lock() = settings;
    }

    #[must_use]
    pub fn voice_settings(&self) -> VoiceSettings {
        self.voice.lock().clone()
    }

    pub fn replace_voice_settings(&self, settings: VoiceSettings) {
        self.with_voice(|voice| voice.replace_settings(settings.clone()));
        *self.voice.lock() = settings;
    }

    #[must_use]
    pub fn character_settings(&self) -> CharacterSettings {
        self.characters.lock().clone()
    }

    pub fn replace_character_settings(&self, settings: CharacterSettings) {
        *self.characters.lock() = settings;
    }

    pub fn mark_proactive(&self, session: SessionId) {
        self.last_proactive.lock().insert(session, Instant::now());
    }

    #[must_use]
    pub fn last_proactive(&self, session: SessionId) -> Option<Instant> {
        self.last_proactive.lock().get(&session).copied()
    }

    #[must_use]
    pub fn observation(&self) -> &parking_lot::Mutex<ene_work::ObservationPipeline> {
        &self.observation
    }

    #[must_use]
    pub fn world_state(&self) -> &parking_lot::Mutex<ene_companion::WorldStateMemory> {
        &self.world_state
    }

    /// Spawn harness tools, provider plugins, and handwritten MCP rows.
    pub async fn apply_plugin_profile(&self) {
        let ai = self.ai.lock().clone();
        let plugins = self.plugins.lock().clone();
        let home = plugins.resolved_home(&self.data_dir);
        if let Err(err) = std::fs::create_dir_all(&home) {
            tracing::warn!(error = %err, path = %home.display(), "plugin home_dir not created");
        }
        self.supervisor.set_plugin_runtime(
            home,
            plugins.ipc.max_frame_bytes,
            plugins.policy.allow_unverified,
        );
        let mut rows =
            crate::plugin_profile::collect_rows(&self.data_dir, &self.work, &ai, &plugins);
        crate::plugin_profile::overlay_persisted_config(&mut rows, &self.data_dir, &self.vault);
        let report = self.supervisor.apply_profile(&rows).await;
        if !report.waiting.is_empty() {
            tracing::warn!(waiting = ?report.waiting, "plugin profile rows waiting");
        }
    }

    pub fn mcp_servers(&self) -> Vec<ene_work::McpServer> {
        crate::plugin_profile::load_servers(&self.data_dir, &self.work)
    }

    pub fn replace_mcp_servers(&self, servers: &[ene_work::McpServer]) -> Result<(), CoreError> {
        crate::plugin_profile::save_servers(&self.data_dir, &self.work, servers)?;
        Ok(())
    }

    pub fn replace_ai(&self, ai: AiSettings, secrets: TaskSecrets) {
        *self.ai.lock() = ai;
        store_secret(&self.chat_secret, secrets.chat);
        store_secret(&self.classifier_secret, secrets.classifier);
        store_secret(&self.embedding_secret, secrets.embedding);
        store_secret(&self.proactive_secret, secrets.proactive);
        store_secret(&self.tts_secret, secrets.tts);
        store_secret(&self.stt_secret, secrets.stt);
        store_secret(&self.approve_secret, secrets.approve);
        store_secret(&self.job_secret, secrets.job);
    }

    pub fn replace_plugins(&self, plugins: PluginSettings) {
        if let Some(mode) = ApprovalMode::parse(&plugins.policy.approval_mode)
            && let Err(err) = self.plane.set_mode(mode)
        {
            tracing::warn!(error = %err, "failed to apply live approval mode");
        }
        *self.plugins.lock() = plugins;
    }

    #[must_use]
    pub fn store(&self) -> Arc<SessionStore> {
        Arc::clone(&self.store)
    }

    #[must_use]
    pub fn recovery(&self) -> &[RecoveryReport] {
        &self.recovery
    }

    #[must_use]
    pub fn supervisor(&self) -> Arc<Supervisor> {
        Arc::clone(&self.supervisor)
    }

    #[must_use]
    pub fn loop_hooks(&self) -> LoopHooks {
        self.loop_hooks.clone()
    }

    #[must_use]
    pub fn plane(&self) -> Arc<ApprovalPlane> {
        Arc::clone(&self.plane)
    }

    #[must_use]
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    #[must_use]
    pub fn companions(&self) -> Arc<CompanionStore> {
        Arc::clone(&self.companions)
    }

    #[must_use]
    pub fn companion(&self) -> Arc<CompanionRuntime> {
        Arc::clone(&self.companion)
    }

    #[must_use]
    pub fn memory_embed(&self) -> Arc<SlotQueryEmbed> {
        Arc::clone(&self.memory_embed)
    }

    #[must_use]
    pub fn stage(&self) -> Arc<Stage> {
        Arc::clone(&self.stage)
    }

    pub fn with_voice<R>(&self, f: impl FnOnce(&mut VoiceRuntime) -> R) -> R {
        let mut voice = self.stage.voice().lock();
        f(&mut voice)
    }

    #[must_use]
    pub fn occupants(&self) -> Vec<(SoulId, Option<BodyId>)> {
        self.stage.occupants()
    }

    #[must_use]
    pub fn work(&self) -> Arc<WorkStore> {
        Arc::clone(&self.work)
    }

    #[must_use]
    pub fn host(&self) -> Arc<DelegationHost> {
        Arc::clone(&self.host)
    }

    #[must_use]
    pub fn job_reports(&self) -> &[CompanionReport] {
        &self.job_reports
    }

    #[must_use]
    pub fn popup(&self) -> &Arc<PendingPopup> {
        &self.popup
    }

    /// Checkpoint databases, close the session writer, and copy a backup generation on disk.
    pub async fn prepare_restore(&self, backup_id: &str) -> Result<(), CoreError> {
        use crate::http::backup::{checkpoint_db, restore_copy, validate_restore_id};
        validate_restore_id(backup_id).map_err(|err| CoreError::Http(err.0.title.clone()))?;
        checkpoint_db(&self.data_dir.join("sessions.db"))
            .map_err(|err| CoreError::Http(err.0.title.clone()))?;
        checkpoint_db(&self.data_dir.join("companions.db"))
            .map_err(|err| CoreError::Http(err.0.title.clone()))?;
        checkpoint_db(&self.data_dir.join("audit.db"))
            .map_err(|err| CoreError::Http(err.0.title.clone()))?;
        self.store.close_writer().await?;
        restore_copy(&self.data_dir, backup_id)
            .map_err(|err| CoreError::Http(err.0.title.clone()))?;
        Ok(())
    }

    /// Reopen stores after [`Self::prepare_restore`].
    pub async fn finish_restore(&self) -> Result<(), CoreError> {
        let sync = self.store_settings.lock().sessions.synchronous.clone();
        self.store.reopen_writer().await?;
        self.store.reload_reader(&sync)?;
        self.companions.reconnect()?;
        self.work.reconnect()?;
        self.plane.audit().reconnect()?;
        Ok(())
    }

    /// Bind a soul to a body (or text-only) and map affect onto the bus (D-19).
    pub fn present_companion(
        &self,
        soul: ene_session::SoulId,
        body: Option<ene_session::BodyId>,
        catalog: BodyCatalog,
    ) -> Result<(), CoreError> {
        self.stage.present(soul, body, catalog)?;
        Ok(())
    }

    pub fn apply_body_emotion(
        &self,
        soul: ene_session::SoulId,
        cue: &EmotionCue,
    ) -> Result<(), CoreError> {
        self.stage.apply_emotion(soul, cue)?;
        Ok(())
    }

    /// Note injected into the next surface turn (D-13). `None` when nothing was interrupted.
    #[must_use]
    pub fn interruption_note(&self) -> Option<String> {
        let session = format_recovery_note(&self.recovery);
        if self.job_reports.is_empty() {
            return session;
        }
        let jobs = self
            .job_reports
            .iter()
            .map(|report| report.speech.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Some(match session {
            Some(session) => format!("{session} {jobs}"),
            None => jobs,
        })
    }

    /// Conversations whose last event is older than `store.sessions.idle_timeout_secs`.
    ///
    /// Does not write `session/end`; the HTTP layer stops the turn first.
    pub fn list_idle_sessions(&self) -> Result<Vec<SessionId>, CoreError> {
        self.list_idle_sessions_since(self.idle_timeout_secs())
    }

    fn list_idle_sessions_since(&self, timeout_secs: u64) -> Result<Vec<SessionId>, CoreError> {
        if timeout_secs == 0 {
            return Ok(Vec::new());
        }
        let now = chrono::Utc::now();
        let mut idle = Vec::new();
        for meta in self.store.list_sessions(None)? {
            if meta.ended_at.is_some() {
                continue;
            }
            let Some(ts) = self.store.last_event_ts(meta.id)? else {
                continue;
            };
            let Ok(then) = chrono::DateTime::parse_from_rfc3339(&ts) else {
                continue;
            };
            let age = now.signed_duration_since(then.with_timezone(&chrono::Utc));
            if age.num_seconds() < i64::try_from(timeout_secs).unwrap_or(i64::MAX) {
                continue;
            }
            idle.push(meta.id);
        }
        Ok(idle)
    }

    fn idle_timeout_secs(&self) -> u64 {
        *self.idle_timeout_secs.lock()
    }

    #[cfg(test)]
    pub(crate) fn set_idle_timeout_secs(&self, secs: u64) {
        *self.idle_timeout_secs.lock() = secs;
    }

    /// Write `session/end`. Callers must stop any in-flight turn first.
    pub async fn end_session(
        &self,
        session: SessionId,
        reason: SessionEndReason,
    ) -> Result<(), CoreError> {
        let meta = self.store.get_session(session)?;
        if meta.ended_at.is_some() {
            return Ok(());
        }
        self.store
            .commit(Transaction {
                entries: vec![NewEvent::new(
                    session,
                    EventKind::SessionEnd,
                    EventPayload::SessionEnd {
                        v: v1(),
                        reason,
                        summary_ref: None,
                    },
                )],
                usage: Vec::new(),
            })
            .await?;
        Ok(())
    }

    /// Open a dialogue lane on an existing session, carrying recovery into context.
    #[must_use]
    pub fn open_lane(
        &self,
        soul: SoulId,
        session: SessionId,
        model: Arc<dyn ConversationModel>,
    ) -> LaneHandle {
        let harness = self.harness.lock().clone();
        let mind = self.mind.lock().clone();
        let router = Arc::new(WorkSurfaceRouter::new(
            Arc::clone(&self.host),
            self.supervisor.registry(),
            soul,
            harness.loop_cfg.max_steps_per_turn,
        ));
        LaneHandle::spawn(LaneOptions {
            store: Arc::clone(&self.store),
            session,
            soul,
            model,
            harness,
            mind: LaneMindSettings::from_inner_window(
                mind.inner.self_reference_window,
                mind.inner.auto_emotion_events,
                mind.inner.derive_from_thinking,
            ),
            recovery: self.recovery.clone(),
            router: Some(router as Arc<dyn SurfaceRouter>),
            speech: self.speech.lock().clone(),
            finalizer: self.finalizer.lock().clone(),
            prefetch: self.prefetch.lock().clone(),
            extra_context: skill_catalog_for_soul(self, soul),
            hooks: Some(self.loop_hooks.clone()),
        })
    }
}

fn skill_catalog_for_soul(core: &CoreDaemon, soul: SoulId) -> Vec<(String, String)> {
    ene_work::skill_catalog_blocks(&core.data_dir().join("skills"), &core.soul_skill_refs(soul))
}

fn seed_default_occupants(companions: &CompanionStore, stage: &Stage) -> Result<(), CoreError> {
    let souls = companions.list_souls()?;
    if souls.is_empty() {
        for character_ref in ["char.alpha@1", "char.beta@1"] {
            let soul = companions.create_soul(&NewSoul {
                character_ref: character_ref.to_owned(),
                body_ref: None,
                voice_ref: None,
                skill_refs: Vec::new(),
                affect_baseline: ene_companion::AffectBaseline::default(),
            })?;
            stage.present(soul.id, None, BodyCatalog::text_default())?;
        }
        return Ok(());
    }
    let mut ranked = souls;
    ranked.sort_by_key(|soul| usize::from(!package_has_avatar(companions, &soul.character_ref)));
    for soul in ranked.into_iter().take(2) {
        let catalog = if package_has_avatar(companions, &soul.character_ref) {
            BodyCatalog::vrm_default()
        } else {
            BodyCatalog::text_default()
        };
        stage.present(soul.id, soul.body_ref, catalog)?;
    }
    Ok(())
}

fn package_has_avatar(companions: &CompanionStore, character_ref: &str) -> bool {
    let Some((id, version)) = character_ref.split_once('@') else {
        return false;
    };
    let Ok(Some(path)) = companions.package_path(id, version) else {
        return false;
    };
    ene_companion::avatar_path_for_install(std::path::Path::new(&path)).is_some()
}

fn lock_data_dir(data_dir: &Path) -> Result<File, CoreError> {
    let path = data_dir.join("ene-core.lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|err| lock_error(&path, err))?;
    file.try_lock_exclusive()
        .map_err(|err| lock_error(&path, err))?;
    Ok(file)
}

fn lock_error(path: &Path, err: std::io::Error) -> CoreError {
    if err.kind() == std::io::ErrorKind::WouldBlock
        || cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33))
    {
        CoreError::AlreadyRunning(path.display().to_string())
    } else {
        CoreError::Io(err)
    }
}

struct BootConfig {
    core: CoreSettings,
    ai: AiSettings,
    plugins: PluginSettings,
    mind: MindSettings,
    harness: HarnessSettings,
    store: StoreSettings,
    approval: ApprovalSettings,
    voice: VoiceSettings,
    body: BodySettings,
    characters: CharacterSettings,
}

fn load_boot_config(data_dir: &Path) -> Result<BootConfig, CoreError> {
    let full = ene_config::load_full_config_from(&data_dir.join("settings.json"))?;
    let mut core = full.get_section::<CoreSettings>()?;
    if core.data_dir.is_empty() {
        core.data_dir = data_dir.display().to_string();
    }
    let plugins = full.get_section::<PluginSettings>()?;
    let mut approval = full.get_section::<ApprovalSettings>()?;
    if !full.extra.contains_key("approval") {
        if let Some(mode) = ApprovalMode::parse(&plugins.policy.approval_mode) {
            approval.mode = mode;
        } else if !plugins.policy.approval_mode.is_empty() {
            tracing::warn!(
                mode = %plugins.policy.approval_mode,
                "unknown plugins.policy.approval_mode; using policy"
            );
        }
    }
    Ok(BootConfig {
        core,
        ai: full.get_section()?,
        plugins,
        mind: full.get_section()?,
        harness: full.get_section()?,
        store: full.get_section()?,
        approval,
        voice: full.get_section()?,
        body: full.get_section()?,
        characters: full.get_section()?,
    })
}

fn resolve_data_subdir(data_dir: &Path, configured: &str, default_name: &str) -> PathBuf {
    if configured.is_empty() {
        data_dir.join(default_name)
    } else {
        let path = PathBuf::from(configured);
        if path.is_absolute() {
            path
        } else {
            data_dir.join(path)
        }
    }
}

/// PATCH/GET allowlist: registered top-level settings sections plus `theme`.
#[must_use]
pub(crate) fn settings_patch_keys() -> Vec<String> {
    let mut keys = ene_config::registered_settings_section_keys();
    if !keys.iter().any(|key| key == "theme") {
        keys.push("theme".to_owned());
    }
    keys.sort();
    keys
}

fn load_named_secret(vault: &Vault, env_key: &str, vault_key: &str) -> String {
    if let Ok(key) = std::env::var(env_key)
        && !key.is_empty()
    {
        drop(vault.put(vault_key, key.as_bytes()));
        return key;
    }
    vault
        .export(vault_key)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// Secrets stripped from a settings PATCH.
#[derive(Debug, Clone, Default)]
pub struct TaskSecrets {
    pub chat: Option<String>,
    pub classifier: Option<String>,
    pub embedding: Option<String>,
    pub proactive: Option<String>,
    pub tts: Option<String>,
    pub stt: Option<String>,
    pub approve: Option<String>,
    pub job: Option<String>,
}

fn store_secret(slot: &parking_lot::Mutex<String>, value: Option<String>) {
    if let Some(secret) = value {
        *slot.lock() = secret;
    }
}

pub(crate) fn overlay_ai(live: &mut AiSettings, incoming: &serde_json::Value) {
    overlay_task(&mut live.tasks.chat, incoming.pointer("/tasks/chat"));
    overlay_task(
        &mut live.tasks.classifier,
        incoming.pointer("/tasks/classifier"),
    );
    overlay_task(
        &mut live.tasks.embedding,
        incoming.pointer("/tasks/embedding"),
    );
    overlay_task(
        &mut live.tasks.proactive,
        incoming.pointer("/tasks/proactive"),
    );
    overlay_task(&mut live.tasks.tts, incoming.pointer("/tasks/tts"));
    overlay_task(&mut live.tasks.stt, incoming.pointer("/tasks/stt"));
    overlay_task(&mut live.tasks.approve, incoming.pointer("/tasks/approve"));
    overlay_task(&mut live.tasks.job, incoming.pointer("/tasks/job"));
}

pub(crate) fn overlay_plugins(live: &mut PluginSettings, incoming: &serde_json::Value) {
    if let Some(profile) = incoming.get("profile").and_then(serde_json::Value::as_str) {
        profile.clone_into(&mut live.profile);
    }
    if let Some(home) = incoming.get("home_dir").and_then(serde_json::Value::as_str) {
        home.clone_into(&mut live.home_dir);
    }
    if let Some(policy) = incoming.get("policy") {
        if let Some(mode) = policy
            .get("approval_mode")
            .and_then(serde_json::Value::as_str)
        {
            mode.clone_into(&mut live.policy.approval_mode);
        }
        if let Some(flag) = policy
            .get("allow_unverified")
            .and_then(serde_json::Value::as_bool)
        {
            live.policy.allow_unverified = flag;
        }
    }
    if let Some(n) = incoming
        .pointer("/ipc/max_frame_bytes")
        .and_then(serde_json::Value::as_u64)
        && let Ok(n) = u32::try_from(n)
    {
        live.ipc.max_frame_bytes = n;
    }
}

fn overlay_task(dst: &mut TaskBinding, value: Option<&serde_json::Value>) {
    let Some(value) = value else {
        return;
    };
    if let Ok(parsed) = serde_json::from_value::<TaskBinding>(value.clone()) {
        *dst = parsed;
    }
}
