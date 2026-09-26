use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_wire_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);
    };
}

macro_rules! string_wire_ref {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
    };
}
pub(crate) use string_wire_ref;

uuid_wire_id!(WireMessageId);
uuid_wire_id!(RequestWireId);
uuid_wire_id!(CommandWireId);
uuid_wire_id!(StreamWireId);
uuid_wire_id!(ConnectionWireId);
uuid_wire_id!(DeviceWireId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientIncarnationId {
    pub counter: u64,
    pub random: u64,
}

string_wire_ref!(CompanionWireRef);
string_wire_ref!(ClientWireRef);
string_wire_ref!(RoundWireId);
string_wire_ref!(ClientLocalId);
string_wire_ref!(TextLangWire);
string_wire_ref!(RevalidationReasonWire);
string_wire_ref!(ViewMarkWire);
string_wire_ref!(WireMessageType);
string_wire_ref!(ManagementTargetWire);
string_wire_ref!(DeletionOperationWireRef);
string_wire_ref!(DeletionStatusCursorWire);
string_wire_ref!(BaseViewMark);
string_wire_ref!(UsageCursorWire);
