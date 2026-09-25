use serde::{Deserialize, Serialize};

use super::envelope::ProtocolVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    UnsupportedMessage,
    UnsupportedFieldValue,
    MissingRequiredField,
    IncompatibleProtocol,
    StaleConnection,
    InvalidHandshakePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IncompatibleProtocol {
    pub host_max: ProtocolVersion,
    pub client_max: ProtocolVersion,
    pub hint: String,
}
