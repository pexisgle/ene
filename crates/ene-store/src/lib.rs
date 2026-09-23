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
#[cfg(any(test, feature = "test-support"))]
mod test_parks;
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
        let parks = Arc::clone(&store.test_parks);
        run_blocking(move || {
            parks
                .deletion_blocking_live
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            struct Live {
                parks: Arc<test_parks::TestParks>,
            }
            impl Drop for Live {
                fn drop(&mut self) {
                    self.parks
                        .deletion_blocking_live
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _live = Live {
                parks: Arc::clone(&parks),
            };
            parks.deletion_blocking.pause_blocking_if_armed();
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
    test_parks: Arc<test_parks::TestParks>,
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
            test_parks: Arc::new(test_parks::TestParks::default()),
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn relax_durability_for_tests(&self) -> Result<(), StoreError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let conn = conn.lock().map_err(|_| {
                StoreError::OpenFailed(String::from("test store connection lock is poisoned"))
            })?;
            conn.pragma_update(None, "journal_mode", "MEMORY")
                .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
            conn.pragma_update(None, "synchronous", "OFF")
                .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
            Ok(())
        })
        .await
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
        self.test_parks
            .deletion_blocking_live
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_erasure_mutation_if_armed_for_tests(&self) {
        self.test_parks.erasure_mutation.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_client_demand_if_armed_for_tests(&self) {
        self.test_parks.client_demand.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_queue_if_armed_for_tests(&self) {
        self.test_parks.host_transient_queue.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_take_if_armed_for_tests(&self) {
        self.test_parks.learning_take.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_formation_if_armed_for_tests(&self) {
        self.test_parks.learning_formation.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks.host_transient_verified_record.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks
            .host_transient_verified_record
            .wait_entered()
            .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks.host_transient_verified_record.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_verified_record_if_armed_for_tests(&self) {
        self.test_parks
            .host_transient_verified_record
            .pause_if_armed()
            .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.wait_entered().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_pin_queue_if_armed_for_tests(&self) {
        self.test_parks.learning_pin_queue.pause_if_armed().await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks.host_transient_arrival_publish.arm();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks
            .host_transient_arrival_publish
            .wait_entered()
            .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks.host_transient_arrival_publish.release();
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_arrival_publish_if_armed_for_tests(&self) {
        self.test_parks
            .host_transient_arrival_publish
            .pause_if_armed()
            .await;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_host_transient_arrivals_until_allow_for_tests(&self) {
        self.test_parks
            .fail_host_transient_arrival_sticky
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn allow_host_transient_arrival_for_tests(&self) {
        self.test_parks
            .fail_host_transient_arrival_sticky
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_deletion_operation_material_for_tests(
        &self,
        operation: ene_preservation::DeletionOperationId,
    ) {
        *self
            .test_parks
            .fail_deletion_material
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(operation);
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn allow_deletion_operation_material_for_tests(&self) {
        *self
            .test_parks
            .fail_deletion_material
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn host_transient_arrival_classify_fails_for_tests(
        &self,
        operation: ene_preservation::DeletionOperationId,
    ) -> bool {
        *self
            .test_parks
            .fail_deletion_material
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == Some(operation)
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn host_transient_arrival_attempts_for_tests(&self) -> u64 {
        self.test_parks
            .host_transient_arrival_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}
