use serde::{Deserialize, Serialize};

use super::refs::{ClientWireRef, CompanionWireRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PresenceStateWire {
    Present,
    NoActive,
    InTransition,
    Stopped,
    RecoveryWait,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceAttributionWire {
    pub companion: CompanionWireRef,
    pub state: PresenceStateWire,
    pub active_client: Option<ClientWireRef>,
    pub generation: u64,
}
