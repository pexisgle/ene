use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    UnsupportedMessage,
    UnsupportedFieldValue,
    MissingRequiredField,
    IncompatibleProtocol,
    ConflictingCommand,
    StaleConnection,
    InvalidHandshakePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
    pub detail: String,
}
