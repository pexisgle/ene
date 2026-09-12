//! Workspace association for one Task.

use ene_primitive::RawId;

use crate::task::TaskId;

/// Identity of one Workspace association. Wraps [`RawId`]; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceAssocId(RawId);

impl WorkspaceAssocId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// Locator of an external folder. ene associates with the folder; it never
/// owns the folder or its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFolderRef {
    pub path: String,
}

/// The workspace conditions a Task creation proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceNeedRef {
    pub folder: WorkspaceFolderRef,
    /// Where durable outputs go. `None` means the Owner is asked before the
    /// final save.
    pub save_target: Option<WorkspaceFolderRef>,
}

/// A confirmed association written as part of the Task creation commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAssociationPremise {
    pub assoc: WorkspaceAssocId,
    pub need: WorkspaceNeedRef,
}

/// The durable association of one Task to an external folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAssociation {
    pub assoc: WorkspaceAssocId,
    pub task: TaskId,
    pub folder: WorkspaceFolderRef,
    pub save_target: Option<WorkspaceFolderRef>,
}
