//! Task-owned instruction-body source port.
//!
//! The canonical body of an adopted instruction is the History record its
//! `origin.source` references; the Task side never copies that body into
//! `task_context_entry`, a delegation, an inference attempt, or a task
//! result. The port is Task-owned so `ene-task` never imports the
//! conversation-history owner's concrete types: the Host composition root
//! implements it against `HistoryRepository::load_message`.
//!
//! The port contract is a single-message bounded read by the canonical
//! source identity. Absence is [`Ok(None)`]; a malformed durable row is an
//! error the caller must fail closed on, never a composed substitute.

use ene_primitive::RawId;

/// The conversation role of one source record, as the Task side needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskInstructionRole {
    /// The partner's Owner; only this role is a valid instruction author.
    Owner,
    /// The Companion itself.
    Companion,
}

/// One resolved instruction source record.
///
/// This carries only what the Task Agent turn needs: the canonical source
/// identity, the speaking companion as an opaque [`RawId`], the mapped role,
/// and the body text. The body stays canonical in History; this value lives
/// only for the turn. [`core::fmt::Debug`] redacts the text.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskInstructionSourceRecord {
    /// The History record identity. The caller verifies it equals the
    /// `origin.source` it resolved.
    pub source: RawId,
    /// The speaking companion in the same [`RawId`] space as
    /// `task.assignee.companion`; never converted into another newtype.
    pub companion: RawId,
    /// The mapped conversation role; the caller requires
    /// [`TaskInstructionRole::Owner`].
    pub role: TaskInstructionRole,
    /// The record body; redacted from [`core::fmt::Debug`].
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

/// Technical failure of the instruction-source read; never body text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskInstructionSourceError {
    /// The canonical source could not be read. The reason is a fixed class,
    /// never the body, a History row, or a secret.
    #[error("task instruction source unavailable: {reason}")]
    SourceUnavailable { reason: String },
}

/// Reads the canonical body of one adopted instruction source.
///
/// The implementation must resolve `source` with a direct single-message
/// read of the History primary key; loading a timeline and searching it,
/// reading a recent window, or resolving a command is not a substitute.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait TaskInstructionSource: Send + Sync {
    /// Loads the source record for `source`, if the canonical row exists.
    ///
    /// Returns `Ok(None)` when the referenced record is absent; it is a domain
    /// fact the caller reports as a missing instruction source, not a
    /// technical failure and not a corruption of the Task context.
    /// A malformed durable row is a [`TaskInstructionSourceError`].
    async fn load_owner_instruction(
        &self,
        source: RawId,
    ) -> Result<Option<TaskInstructionSourceRecord>, TaskInstructionSourceError>;
}

#[cfg(test)]
mod tests {
    use super::TaskInstructionSourceRecord;

    #[test]
    fn source_record_debug_redacts_the_body() {
        let record = TaskInstructionSourceRecord {
            source: ene_primitive::RawId::new(),
            companion: ene_primitive::RawId::new(),
            role: super::TaskInstructionRole::Owner,
            text: String::from("probe instruction body"),
        };
        let rendered = format!("{record:?}");
        assert!(!rendered.contains("probe instruction body"));
    }
}
