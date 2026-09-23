use ene_primitive::RawId;

use crate::context::{TaskContextOrigin, TaskContextOriginKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskInstructionRole {
    Owner,
    Companion,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TaskInstructionSourceRecord {
    pub kind: TaskContextOriginKind,
    pub source: RawId,
    pub companion: RawId,
    pub role: TaskInstructionRole,
    pub text: String,
}

impl core::fmt::Debug for TaskInstructionSourceRecord {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskInstructionSourceRecord")
            .field("source", &self.source)
            .field("companion", &self.companion)
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskInstructionSourceError {
    #[error("task instruction source unavailable: {reason}")]
    SourceUnavailable { reason: String },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait TaskInstructionSource: Send + Sync {
    async fn load_owner_instruction(
        &self,
        origin: TaskContextOrigin,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>;
}
