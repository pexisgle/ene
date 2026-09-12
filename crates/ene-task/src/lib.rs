//! Task ownership: Task identity, purpose, context, workspace association,
//! and the repository boundary that persists them.
//!
//! A [`Task`] is one unit of tracked work. Its identity ([`TaskId`]) is
//! separate from the revision ([`TaskRevision`]) that orders the owner's
//! decisions; the two travel together as a [`TaskRef`]. The adopted purpose
//! is identified by [`TaskPurposeRef`] and its text lives in the revision
//! snapshot, so a purpose change is a revision forward and never a change of
//! another lifecycle's generation.
//!
//! This crate owns the semantics and the [`TaskRepository`] contract; the
//! persistent implementation lives behind that trait (see `ene-store`), so
//! this crate never depends on the store, and it never imports another
//! domain's newtype: cross-domain identities arrive as owner-defined
//! premises.

mod context;
mod repository;
mod task;
mod workspace;

pub use context::{
    TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin, TaskContextOriginKind,
};
pub use repository::{TaskRepository, TaskTechnicalError};
pub use task::{
    AssigneeRef, Task, TaskCreationPremise, TaskId, TaskPurpose, TaskPurposeRef, TaskRecord,
    TaskRef, TaskRevision, TaskRevisionRecord,
};
pub use workspace::{
    WorkspaceAssocId, WorkspaceAssociation, WorkspaceAssociationPremise, WorkspaceFolderRef,
    WorkspaceNeedRef,
};
