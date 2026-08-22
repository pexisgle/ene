use crate::error::SessionError;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Allocate a new `UUIDv7` identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// Borrow the inner UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = SessionError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|_| SessionError::InvalidId(s.to_owned()))
            }
        }
    };
}

uuid_id!(
    /// Conversation or delegation session.
    SessionId
);
uuid_id!(
    /// Companion soul that owns a session.
    SoulId
);
uuid_id!(
    /// Optional body bound to a session.
    BodyId
);
uuid_id!(
    /// One user/companion turn.
    TurnId
);
uuid_id!(
    /// Client connection that originated an input.
    ClientId
);
uuid_id!(
    /// Public or internal delegation (job) identifier.
    DelegationId
);
uuid_id!(
    /// Tool-call correlation id.
    CallId
);
uuid_id!(
    /// Ask-user question identifier.
    QuestionId
);

impl QuestionId {
    /// Stable id for a delegation mailbox question row.
    #[must_use]
    pub fn from_mailbox(delegation: DelegationId, mailbox_seq: i64) -> Self {
        const NS: Uuid = uuid::uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479");
        Self::from_uuid(Uuid::new_v5(
            &NS,
            format!("{delegation}:{mailbox_seq}").as_bytes(),
        ))
    }
}
uuid_id!(
    /// Usage-ledger row identifier.
    UsageId
);
uuid_id!(
    /// Context-epoch identifier.
    EpochId
);
