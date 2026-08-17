use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ene_kernel::{ConversationModel, LaneHandle, LaneOptions, format_recovery_note};
use ene_session::{RecoveryReport, SessionId, SessionStore, SoulId};
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
    #[error("another ene-core instance holds the data-directory lock at {0}")]
    AlreadyRunning(String),
}

/// Running core: exclusive lock, session store, D-5 recovery reports.
pub struct CoreDaemon {
    data_dir: PathBuf,
    _lock: File,
    store: Arc<SessionStore>,
    recovery: Vec<RecoveryReport>,
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
        Ok(Self {
            data_dir: opts.data_dir,
            _lock: lock,
            store: Arc::new(store),
            recovery,
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

    /// Note injected into the next surface turn (D-13). `None` when nothing was interrupted.
    #[must_use]
    pub fn interruption_note(&self) -> Option<String> {
        format_recovery_note(&self.recovery)
    }

    /// Open a dialogue lane on an existing session, carrying recovery into context.
    #[must_use]
    pub fn open_lane(
        &self,
        soul: SoulId,
        session: SessionId,
        model: Arc<dyn ConversationModel>,
    ) -> LaneHandle {
        LaneHandle::spawn(LaneOptions {
            store: Arc::clone(&self.store),
            session,
            soul,
            model,
            harness: ene_kernel::HarnessSettings::default(),
            mind: ene_kernel::MindSettings::default(),
            recovery: self.recovery.clone(),
        })
    }
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
