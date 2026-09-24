use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use rusqlite::Connection;

mod action;
mod client_delivery;
mod codec;
mod companion;
mod credential;
mod credential_publication;
mod erasure;
mod inference;
mod learning;
mod migrate;
mod permission;
mod presence;
mod preservation;
mod task;
mod usage_cap;

pub use companion::UndeliveredExcerpt;
pub use erasure::{
    action_erasure_participant, companion_erasure_participant, inference_erasure_participant,
    learning_erasure_participant, task_erasure_participant,
};
pub use preservation::{HOST_TRANSIENT_ARRIVAL_PAGE, HostTransientArrivalOutcome};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("store open failed: {0}")]
    OpenFailed(String),
    #[error("store schema initialization failed: {0}")]
    SchemaFailed(String),
}

async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => {
            if join.is_panic() {
                std::panic::resume_unwind(join.into_panic());
            }
            std::panic::resume_unwind(Box::new("store blocking task was cancelled"));
        }
    }
}

async fn run_deletion_blocking<T: Send + 'static>(
    store: &Store,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    #[cfg(any(test, feature = "test-support"))]
    {
        let live = Arc::clone(&store.deletion_blocking_live);
        run_blocking(move || {
            live.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            struct Live {
                counter: Arc<std::sync::atomic::AtomicUsize>,
            }
            impl Drop for Live {
                fn drop(&mut self) {
                    self.counter
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _live = Live {
                counter: Arc::clone(&live),
            };
            work()
        })
        .await
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        let _ = store;
        run_blocking(work).await
    }
}

#[derive(Clone)]
pub struct UndeliveredSignal {
    epoch: Arc<tokio::sync::watch::Sender<u64>>,
}

impl UndeliveredSignal {
    fn new() -> Self {
        Self {
            epoch: Arc::new(tokio::sync::watch::channel(0).0),
        }
    }

    fn bump(&self) {
        self.epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.epoch.subscribe()
    }
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    undelivered: UndeliveredSignal,
    #[cfg(any(test, feature = "test-support"))]
    deletion_blocking_live: Arc<std::sync::atomic::AtomicUsize>,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        run_blocking(move || Self::open_sync(&path)).await
    }

    fn open_sync(path: &Path) -> Result<Self, StoreError> {
        let mut conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        conn.pragma_update(None, "secure_delete", "ON")
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::SchemaFailed)?;
        Ok(Self::from_connection(conn))
    }

    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            undelivered: UndeliveredSignal::new(),
            #[cfg(any(test, feature = "test-support"))]
            deletion_blocking_live: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn count_exact_text_remainder_for_tests(
        &self,
        text: &str,
    ) -> Result<u64, StoreError> {
        let conn = Arc::clone(&self.conn);
        let text = text.to_owned();
        run_blocking(move || {
            let guard = crate::codec::lock_shared(&conn);
            erasure::system_remainder(&guard, &text)
                .map_err(|error| StoreError::OpenFailed(error.to_string()))
        })
        .await
    }

    #[must_use]
    pub fn undelivered_wakeup(&self) -> tokio::sync::watch::Receiver<u64> {
        self.undelivered.subscribe()
    }

    fn hint_after_commit<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            self.undelivered.bump();
        }
        result
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn live_deletion_blocking_sections_for_tests(&self) -> usize {
        self.deletion_blocking_live
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}
