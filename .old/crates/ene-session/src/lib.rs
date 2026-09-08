//! Append-only conversation log, usage ledger, and history projection.
//!
//! Event sourcing applies to the conversation log only (D-9). Registers and
//! the effect sandwich are successor designs and are not implemented here.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::panic,
        reason = "unit tests use unwrap/panic for assertions"
    )
)]

mod block;
mod config;
mod error;
mod event;
mod export;
mod ids;
mod inbox;
mod project;
mod store;
mod usage;

pub use block::{Block, InnerAspect};
pub use config::{ExportSettings, SessionsSettings, StoreSettings};
pub use error::SessionError;
pub use event::{
    EventKind, EventPayload, InboxCancelReason, InboxClass, InboxSource, LaneId, LoggedEvent,
    NewEvent, PAYLOAD_VERSION, SessionCreatedBy, SessionEndReason, StepOutcome, ToolStatus,
    TurnOrigin, TurnOutcome, TurnTrigger, v1,
};
pub use export::{ExportedEvent, SessionExport, export_session};
pub use ids::{
    BodyId, CallId, ClientId, DelegationId, EpochId, QuestionId, SessionId, SoulId, TurnId, UsageId,
};
pub use inbox::{InboxItem, OpenTurn, abandoned_inbox, open_turns, unclaimed_inbox};
pub use project::{
    DisplayDepth, InnerVisibility, ProjectOptions, ProjectedHistory, ProjectedMessage, Role,
    ThinkingVisibility, derive_messages, hash_model_visible, hash_projected, surface_leaks_inner,
};
pub use store::{
    CommitResult, NewSession, RecoveryReport, STORAGE_VERSION, SessionKind, SessionMeta,
    SessionStore, SpillObject, Transaction,
};
pub use usage::{NewUsage, UsageRow, UsageTotals};

#[cfg(test)]
mod tests;
