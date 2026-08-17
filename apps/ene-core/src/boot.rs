use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ene_body::{
    BodyCatalog, BodyError, BodySettings, EmotionCue, PerformanceBus, Stage, VoiceRuntime,
    VoiceSettings,
};
use ene_companion::{
    CompanionRuntime, CompanionStore, MindSettings as CompanionMind, NewSoul, register_memory_tools,
};
use ene_fiber::Supervisor;
use ene_kernel::{ConversationModel, LaneHandle, LaneOptions, SurfaceRouter, format_recovery_note};
use ene_plane::{ApprovalPlane, ApprovalSettings, AuditLog, PendingPopup, PopupSink, Vault};
use ene_registry::ToolRegistry;
use ene_session::{
    BodyId, EventKind, EventPayload, NewEvent, RecoveryReport, SessionEndReason, SessionId,
    SessionStore, SessionsSettings, SoulId, Transaction, v1,
};
use ene_work::{
    CompanionReport, DelegationHost, PlaceholderScreenshot, WorkError, WorkStore,
    WorkSurfaceRouter, register_screenshot_tool, register_work_tools,
};
use fs2::FileExt;
use thiserror::Error;
use tracing::info;

/// Options for booting the core daemon (W0: store + recovery, no HTTP).
#[derive(Debug, Clone)]
pub struct BootOptions {
    /// Data directory that holds `sessions.db` and the lock file.
    pub data_dir: PathBuf,
    /// `SQLite` `synchronous` pragma (`NORMAL` or `FULL`).
    pub synchronous: String,
}

impl BootOptions {
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            synchronous: "NORMAL".to_owned(),
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
    companion: CompanionRuntime,
    stage: Arc<Stage>,
    work: Arc<WorkStore>,
    host: Arc<DelegationHost>,
    job_reports: Vec<CompanionReport>,
    popup: Arc<PendingPopup>,
}

impl CoreDaemon {
    /// Ensure the data dir, take the exclusive lock, open the store, recover interrupts.
    pub async fn boot(opts: BootOptions) -> Result<Self, CoreError> {
        std::fs::create_dir_all(&opts.data_dir)?;
        let lock = lock_data_dir(&opts.data_dir)?;
        let db_path = opts.data_dir.join("sessions.db");
        let store = SessionStore::open(&db_path, &opts.synchronous).await?;
        let recovery = store.recover_interrupted().await?;
        if !recovery.is_empty() {
            info!(
                sessions = recovery.len(),
                "detected interrupted work; closed without resume"
            );
        }
        let registry = Arc::new(ToolRegistry::new());
        registry.set_workspace(opts.data_dir.clone());
        let audit = AuditLog::open(opts.data_dir.join("audit.db"))?;
        let popup = Arc::new(PendingPopup::new());
        let plane = Arc::new(ApprovalPlane::new(
            ApprovalSettings::default(),
            audit,
            Arc::clone(&popup) as Arc<dyn PopupSink>,
            None,
        ));
        registry.set_plane(Arc::clone(&plane));
        let passphrase =
            std::env::var("ENE_VAULT_PASSPHRASE").unwrap_or_else(|_| "local".to_owned());
        let vault = Vault::open_file(opts.data_dir.join("vault.bin"), &passphrase)?;
        let companions = Arc::new(CompanionStore::open(opts.data_dir.join("companions.db"))?);
        register_memory_tools(&registry, Arc::clone(&companions));
        let work = Arc::new(WorkStore::open(opts.data_dir.join("companions.db"))?);
        let host = Arc::new(DelegationHost::new(
            Arc::clone(&work),
            opts.data_dir.clone(),
        ));
        register_work_tools(&registry, Arc::clone(&host), opts.data_dir.join("skills"));
        register_screenshot_tool(&registry, Arc::new(PlaceholderScreenshot));
        let job_reports = host.recover_interrupted()?;
        if !job_reports.is_empty() {
            info!(
                jobs = job_reports.len(),
                "detected interrupted tasks; closed without resume"
            );
        }
        let companion = CompanionRuntime::new(Arc::clone(&companions), CompanionMind::default());
        let bus = Arc::new(PerformanceBus::default());
        let stage = Arc::new(Stage::new(
            bus,
            VoiceRuntime::scripted(VoiceSettings::default()),
            BodySettings::default(),
        ));
        let supervisor = Arc::new(Supervisor::new(opts.data_dir.clone(), registry));
        seed_default_occupants(&companions, &stage)?;
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
        })
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
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
    pub fn companion(&self) -> &CompanionRuntime {
        &self.companion
    }

    #[must_use]
    pub fn stage(&self) -> Arc<Stage> {
        Arc::clone(&self.stage)
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

    /// End conversations whose last event is older than `store.sessions.idle_timeout_secs`.
    pub async fn end_idle_sessions(&self) -> Result<u32, CoreError> {
        let timeout_secs = SessionsSettings::default().idle_timeout_secs;
        if timeout_secs == 0 {
            return Ok(0);
        }
        let now = chrono::Utc::now();
        let mut ended = 0_u32;
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
            self.end_session(meta.id, SessionEndReason::IdleTimeout)
                .await?;
            ended = ended.saturating_add(1);
        }
        Ok(ended)
    }

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
        let harness = ene_kernel::HarnessSettings::default();
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
            mind: ene_kernel::MindSettings::default(),
            recovery: self.recovery.clone(),
            router: Some(router as Arc<dyn SurfaceRouter>),
        })
    }
}

fn seed_default_occupants(companions: &CompanionStore, stage: &Stage) -> Result<(), CoreError> {
    if !companions.list_souls()?.is_empty() {
        return Ok(());
    }
    for character_ref in ["char.alpha@1", "char.beta@1"] {
        let soul = companions.create_soul(&NewSoul {
            character_ref: character_ref.to_owned(),
            body_ref: Some(BodyId::new()),
            voice_ref: None,
            skill_refs: Vec::new(),
            affect_baseline: ene_companion::AffectBaseline::default(),
        })?;
        stage.present(soul.id, soul.body_ref, BodyCatalog::vrm_default())?;
    }
    Ok(())
}

fn lock_data_dir(data_dir: &Path) -> Result<File, CoreError> {
    let path = data_dir.join("ene-core.lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    file.try_lock_exclusive().map_err(|err| {
        if err.kind() == std::io::ErrorKind::WouldBlock {
            CoreError::AlreadyRunning(path.display().to_string())
        } else {
            CoreError::Io(err)
        }
    })?;
    Ok(file)
}
