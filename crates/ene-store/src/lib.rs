//! SQLite-backed implementations of the repository contracts owned by
//! [`ene_presence`], [`ene_companion`], [`ene_permission`],
//! [`ene_credential`], [`ene_inference`], [`ene_learning`], [`ene_task`], and
//! [`ene_action`]; those owners never depend on this crate and program against
//! their own traits. It also implements the preservation-owned local-erasure
//! participants for the owners whose durable master lives here
//! ([`TaskErasureParticipant`], [`ActionErasureParticipant`], and
//! [`InferenceErasureParticipant`]); the Host composition registers them.
//!
//! Concurrency shape: the connection is `Send` but not `Sync`, so an
//! `Arc<std::sync::Mutex<Connection>>` shares it across callers. Each
//! repository method hands its whole critical section — lock, one short
//! [`rusqlite::TransactionBehavior::Immediate`] transaction (or one plain
//! statement for pure loads), drop the guard — to `run_blocking`, so the
//! synchronous `rusqlite` work happens on the blocking pool instead of on an
//! async worker. The guard and any transaction never cross an `.await`: they
//! live and die inside the blocking closure. Values that cross the boundary
//! are bound parameters, never interpolated into SQL text.

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
#[cfg(test)]
mod tests;
mod usage_cap;

pub use companion::UndeliveredExcerpt;
pub use erasure::{
    ActionErasureParticipant, CompanionErasureParticipant, ERASURE_SCAN_ROWS,
    InferenceErasureParticipant, LearningErasureParticipant, TaskErasureParticipant,
};
pub use preservation::{HOST_TRANSIENT_ARRIVAL_PAGE, HostTransientArrivalOutcome};

/// Messages carry the short backend cause only. Paths are non-secret but are
/// kept out of messages for operational brevity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("store open failed: {0}")]
    OpenFailed(String),
    #[error("store schema initialization failed: {0}")]
    SchemaFailed(String),
}

/// A panic inside the blocking task is the task's own panic: resume it rather
/// than reporting it as a store failure.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(join) => std::panic::resume_unwind(join.into_panic()),
    }
}

/// Targeted Deletion Store work the serving driver awaits.
///
/// Under test-support this counts the started `spawn_blocking` section and
/// honours the blocking park *inside* that section, so aborting the awaiting
/// async task cannot skip past already-started SQLite work. The serving
/// tick joins this await chain; graceful shutdown therefore waits for a
/// started section instead of aborting it.
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

/// Coalesced wakeup hint for the presentation subscription (CCT §10.5).
///
/// The store bumps the epoch after any commit that may have inserted an
/// `undelivered` row. The hint carries no state and is never authority:
/// subscribers re-read durable rows after every change, so a duplicated,
/// early, or rolled-back hint is harmless and a lost one only delays
/// delivery until the next hint or connection event. Reads of the
/// `undelivered` table serialize on the store's connection mutex, so a
/// durable query started after a bump always observes the committed row.
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

    /// Subscribes to the coalesced hint. A receiver observes only changes
    /// after it was created; callers that must not lose a registration
    /// subscribe before reading the durable backlog.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.epoch.subscribe()
    }
}

/// SQLite-backed host for every repository contract.
///
/// Cloning is a cheap handle copy over the same connection: the connection
/// table's close admission moves one clone into its `spawn_blocking` section
/// while the owning handle keeps the store.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// Bumped after any commit that may have registered an undelivered row;
    /// see [`UndeliveredSignal`].
    undelivered: UndeliveredSignal,
    /// First-waiter parks for production mutation races. Compiled out of
    /// production binaries; tests arm them on the same `Store` handle the
    /// Host composition clones into participants.
    #[cfg(any(test, feature = "test-support"))]
    test_parks: Arc<test_parks::TestParks>,
}

impl Store {
    /// Initializes an empty database or opens the exact current schema.
    /// Unsupported schemas are rejected without changes.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        run_blocking(move || Self::open_sync(&path)).await
    }

    fn open_sync(path: &Path) -> Result<Self, StoreError> {
        let mut conn =
            Connection::open(path).map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        // Every erase/redaction path runs on this one connection, and SQLite
        // does not overwrite freed pages by default: deleted or redacted text
        // could otherwise survive in the file's free space. The setting is
        // per-connection and must be established before any deletion, not
        // inside an individual erase transaction.
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

    /// Relaxes SQLite durability for test fixtures while keeping the database
    /// file, schema, transaction boundaries, and reopen behavior intact.
    ///
    /// Tests that exercise logical repository/Host behavior do not need an
    /// `fsync` after every short transaction. The explicit opt-in keeps the
    /// production open path unchanged while avoiding that filesystem cost on
    /// Windows CI. The resulting database remains readable by a later normal
    /// [`Store::open`]; this helper only weakens crash/power-loss durability of
    /// the current test connection.
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

    /// In-memory store for this crate's tests only; production opens files.
    #[cfg(test)]
    pub(crate) async fn open_in_memory() -> Result<Self, StoreError> {
        run_blocking(Self::open_in_memory_sync).await
    }

    #[cfg(test)]
    fn open_in_memory_sync() -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|error| StoreError::OpenFailed(error.to_string()))?;
        migrate::run(&mut conn).map_err(StoreError::SchemaFailed)?;
        Ok(Self::from_connection(conn))
    }

    /// Mechanical exact-text remainder probe over the closed system-wide
    /// canonical content surface the A5 completion boundary verifies, plus
    /// the derived token index and the undelivered references whose canonical
    /// source is gone.
    ///
    /// Test-support only: tests assert `0` after an erasure instead of
    /// re-implementing the column list. The list is the same closed surface
    /// `crate::erasure::system_remainder` uses, so a probe cannot check a
    /// different column set than the completion boundary verifies.
    ///
    /// # Errors
    ///
    /// [`StoreError::OpenFailed`] when the connection cannot be locked.
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
            erasure::exact_remainder_probe(&guard, &text)
                .map_err(|error| StoreError::OpenFailed(error.to_string()))
        })
        .await
    }

    /// Subscribes to the coalesced undelivered-registration hint (CCT §10.5).
    ///
    /// Subscribe before reading the durable backlog: a registration that
    /// commits between the read and the wait then changes the epoch, so the
    /// waiter wakes instead of missing the row.
    #[must_use]
    pub fn undelivered_wakeup(&self) -> tokio::sync::watch::Receiver<u64> {
        self.undelivered.subscribe()
    }

    /// Bumps the undelivered hint after `result`, and only when it succeeded.
    ///
    /// Call with the result of a commit that may have inserted an
    /// `undelivered` row. A failed commit rolled back, so there is nothing
    /// new to deliver; a successful one may have, and a spurious bump is
    /// harmless because the hint is never authority.
    fn hint_after_commit<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            self.undelivered.bump();
        }
        result
    }

    /// How many Targeted Deletion `spawn_blocking` sections are currently
    /// inside the instrumented Store boundary. Observation only: production
    /// correctness does not read this.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn live_deletion_blocking_sections_for_tests(&self) -> usize {
        self.test_parks
            .deletion_blocking_live
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Arms the first-waiter park *inside* a Targeted Deletion
    /// `spawn_blocking` section.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.arm();
    }

    /// Waits until the armed deletion-blocking park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.wait_entered().await;
    }

    /// Releases the parked deletion `spawn_blocking` section.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_deletion_blocking_park_for_tests(&self) {
        self.test_parks.deletion_blocking.release();
    }

    /// Arms the first-waiter park just before a Task Agent observation row is
    /// written. Production never calls this; tests use it to hold the
    /// body-in-memory window closed by the in-flight Action correspondence.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.arm();
    }

    /// Waits until the armed observation-write park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.wait_entered().await;
    }

    /// Releases the parked observation write.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_observation_write_park_for_tests(&self) {
        self.test_parks.observation_write.release();
    }

    /// Arms the first-waiter park just before a durable erasure mutation.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.arm();
    }

    /// Waits until the armed erasure-mutation park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.wait_entered().await;
    }

    /// Releases the parked erasure mutation.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_erasure_mutation_park_for_tests(&self) {
        self.test_parks.erasure_mutation.release();
    }

    /// Arms the first-waiter park just before the credential device-auth
    /// file mutation. The metadata transaction has already committed; this
    /// is the non-rollbackable file writer.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.arm();
    }

    /// Waits until the armed device-auth file park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.wait_entered().await;
    }

    /// Releases the parked device-auth file mutation.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_device_auth_file_park_for_tests(&self) {
        self.test_parks.device_auth_file.release();
    }

    /// Arms the first-waiter park just before a Client class-wipe demand is
    /// created in the in-process registry (before it can reach the wire).
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.arm();
    }

    /// Waits until the armed Client-demand park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.wait_entered().await;
    }

    /// Releases the parked Client demand.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_client_demand_park_for_tests(&self) {
        self.test_parks.client_demand.release();
    }

    /// Arms the first-waiter park after HostTransient snapshots the Learning
    /// formation queue and before the covered-source membership probe.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.arm();
    }

    /// Waits until the armed HostTransient queue-snapshot park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.wait_entered().await;
    }

    /// Releases the parked HostTransient queue snapshot.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_queue_park_for_tests(&self) {
        self.test_parks.host_transient_queue.release();
    }

    /// Arms the first-waiter park after a Learning worker takes a candidate
    /// off the pending queue and before `begin_learning_formation`.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.arm();
    }

    /// Waits until the armed Learning-take park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.wait_entered().await;
    }

    /// Releases the parked Learning take.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_take_park_for_tests(&self) {
        self.test_parks.learning_take.release();
    }

    /// Arms the first-waiter park after a Learning formation identity is
    /// published and before the inference claim is created.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.arm();
    }

    /// Waits until the armed Learning-formation park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.wait_entered().await;
    }

    /// Releases the parked Learning formation pass.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_formation_park_for_tests(&self) {
        self.test_parks.learning_formation.release();
    }

    /// Pauses when the erasure-mutation park is armed. Called from
    /// production participant paths that live outside this crate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_erasure_mutation_if_armed_for_tests(&self) {
        self.test_parks.erasure_mutation.pause_if_armed().await;
    }

    /// Pauses when the Client-demand park is armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_client_demand_if_armed_for_tests(&self) {
        self.test_parks.client_demand.pause_if_armed().await;
    }

    /// Pauses when the HostTransient queue-snapshot park is armed. Called
    /// after the pending page and generation are snapshotted and before the
    /// membership probe, so a test can mutate the queue without holding the
    /// queue mutex across an await.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_queue_if_armed_for_tests(&self) {
        self.test_parks.host_transient_queue.pause_if_armed().await;
    }

    /// Pauses when the Learning-take park is armed. Called from the
    /// production worker after `take_pending` and before
    /// `begin_learning_formation`.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_take_if_armed_for_tests(&self) {
        self.test_parks.learning_take.pause_if_armed().await;
    }

    /// Pauses when the Learning-formation park is armed. Called from the
    /// production worker after the body-free formation identity is published
    /// and before the Learning inference claim.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_formation_if_armed_for_tests(&self) {
        self.test_parks.learning_formation.pause_if_armed().await;
    }

    /// Pauses when the device-auth file park is armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_device_auth_file_if_armed_for_tests(&self) {
        self.test_parks.device_auth_file.pause_if_armed().await;
    }

    /// Arms the first-waiter park after HostTransient has minted a Verified
    /// fact and before that fact is recorded durably.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks.host_transient_verified_record.arm();
    }

    /// Waits until the armed HostTransient verified-record park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks
            .host_transient_verified_record
            .wait_entered()
            .await;
    }

    /// Releases the parked HostTransient verified-record commit.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_verified_record_park_for_tests(&self) {
        self.test_parks.host_transient_verified_record.release();
    }

    /// Pauses when the HostTransient verified-record park is armed. Called
    /// from the composition commit helper after a Verified fact exists and
    /// before the arrival gate is taken.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_verified_record_if_armed_for_tests(&self) {
        self.test_parks
            .host_transient_verified_record
            .pause_if_armed()
            .await;
    }

    /// Arms the first-waiter park at the start of the sealed finalizing
    /// boundary, before the HostTransient arrival gate is taken.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_deletion_finalizing_park_for_tests(&self) {
        self.test_parks.deletion_finalizing.arm();
    }

    /// Waits until the armed deletion-finalizing park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_deletion_finalizing_park_for_tests(&self) {
        self.test_parks.deletion_finalizing.wait_entered().await;
    }

    /// Releases the parked deletion-finalizing attempt.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_deletion_finalizing_park_for_tests(&self) {
        self.test_parks.deletion_finalizing.release();
    }

    /// Pauses when the deletion-finalizing park is armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_deletion_finalizing_if_armed_for_tests(&self) {
        self.test_parks.deletion_finalizing.pause_if_armed().await;
    }

    /// Arms the first-waiter park after Dialogue has pinned an
    /// `ExperienceCandidate` and before that candidate is handed to the
    /// Learning formation queue.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.arm();
    }

    /// Waits until the armed pin-to-queue park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.wait_entered().await;
    }

    /// Releases the parked pin-to-queue handoff.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_learning_pin_queue_park_for_tests(&self) {
        self.test_parks.learning_pin_queue.release();
    }

    /// Pauses when the pin-to-queue park is armed. Called after occupancy is
    /// registered and `pin_experience` has produced the candidate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_learning_pin_queue_if_armed_for_tests(&self) {
        self.test_parks.learning_pin_queue.pause_if_armed().await;
    }

    /// Arms the first-waiter park after a Learning candidate is on the queue
    /// and before canonical HostTransient arrival publication.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn arm_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks.host_transient_arrival_publish.arm();
    }

    /// Waits until the armed arrival-publish park has a waiter.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn wait_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks
            .host_transient_arrival_publish
            .wait_entered()
            .await;
    }

    /// Releases the parked arrival-publish handoff.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn release_host_transient_arrival_publish_park_for_tests(&self) {
        self.test_parks.host_transient_arrival_publish.release();
    }

    /// Pauses when the arrival-publish park is armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn pause_host_transient_arrival_publish_if_armed_for_tests(&self) {
        self.test_parks
            .host_transient_arrival_publish
            .pause_if_armed()
            .await;
    }

    /// Forces the next `note_host_transient_learning_arrival` to fail closed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_next_host_transient_arrival_for_tests(&self) {
        self.test_parks
            .fail_host_transient_arrival
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Forces every `note_host_transient_learning_arrival` to fail until
    /// [`Self::allow_host_transient_arrival_for_tests`].
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_host_transient_arrivals_until_allow_for_tests(&self) {
        self.test_parks
            .fail_host_transient_arrival_sticky
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Clears a forced HostTransient arrival publication failure.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn allow_host_transient_arrival_for_tests(&self) {
        self.test_parks
            .fail_host_transient_arrival_sticky
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.test_parks
            .fail_host_transient_arrival
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Forces HostTransient arrival classification for one operation to fail
    /// until [`Self::allow_deletion_operation_material_for_tests`].
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

    /// Clears a forced HostTransient arrival classification failure.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn allow_deletion_operation_material_for_tests(&self) {
        *self
            .test_parks
            .fail_deletion_material
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Whether arrival classification for `operation` is forced to fail.
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

    /// How many times `note_host_transient_learning_arrival` has been entered.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn host_transient_arrival_attempts_for_tests(&self) -> u64 {
        self.test_parks
            .host_transient_arrival_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}
