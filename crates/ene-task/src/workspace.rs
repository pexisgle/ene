use ene_primitive::RawId;

use crate::task::TaskId;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFolderRef {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceNeedRef {
    pub folder: WorkspaceFolderRef,
    pub save_target: Option<WorkspaceFolderRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAssociationPremise {
    pub assoc: WorkspaceAssocId,
    pub need: WorkspaceNeedRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAssociation {
    pub assoc: WorkspaceAssocId,
    pub task: TaskId,
    pub folder: WorkspaceFolderRef,
    pub save_target: Option<WorkspaceFolderRef>,
}
