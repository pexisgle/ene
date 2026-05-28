use serde::{Deserialize, Serialize};

macro_rules! define_id_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            /// Creates a new instance from a string.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// Returns the inner string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes self and returns the inner String.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

define_id_type!(SessionId, "Unique session identifier");
define_id_type!(CardName, "Character card name");
define_id_type!(ToolName, "Tool identifier");
define_id_type!(RequestId, "Permission or IPC request identifier");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_roundtrip() {
        let id = SessionId::new("session_abc123");
        assert_eq!(id.as_str(), "session_abc123");
        assert_eq!(format!("{id}"), "session_abc123");
        assert_eq!(id, SessionId::from("session_abc123"));
    }

    #[test]
    fn tool_name_from_string() {
        let name: ToolName = "read_file".to_string().into();
        assert_eq!(name.as_str(), "read_file");
    }

    #[test]
    fn card_name_into_inner() {
        let name = CardName::new("alice");
        assert_eq!(name.into_inner(), "alice");
    }
}
