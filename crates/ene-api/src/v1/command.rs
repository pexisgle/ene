use serde::{Deserialize, Serialize};

use super::refs::CommandWireId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum CommandReplayRejectWire {
    CommandIdConflict { command_id: CommandWireId },
    AlreadyProcessed { command_id: CommandWireId },
}
