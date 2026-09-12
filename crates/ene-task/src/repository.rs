//! The durable boundary for Task creation and reload.

use thiserror::Error;

use crate::task::{TaskCreationPremise, TaskId, TaskRecord, TaskRef};

/// Infrastructure failure for Task persistence.
///
/// Domain acceptance is never this error; missing identities are [`None`]
/// on the `Ok` side of reads.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskTechnicalError {
    #[error("task storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause. Never Task content.
        reason: String,
    },
}

/// Durable Task boundary.
///
/// [`Self::create_task`] writes the current Task row, its initial revision
/// record, the initial context entries, and a confirmed workspace
/// association in one atomic section: the commit is the only visibility
/// boundary, and a failed insert leaves no partial row behind.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait TaskRepository: Send + Sync {
    /// Creates one Task at its initial revision and returns its reference.
    ///
    /// The caller mints the identities in the premise. `workspace` is
    /// `None` when the Task has no confirmed workspace association.
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError>;

    /// Loads the committed AU2 unit of one Task at its current revision.
    ///
    /// `None` means the identity has no stored Task. Partial or inconsistent
    /// rows are never composed into a [`TaskRecord`]; that is a technical
    /// error.
    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError>;
}
