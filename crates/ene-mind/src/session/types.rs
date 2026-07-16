use serde::{Deserialize, Serialize};

macro_rules! define_id_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            /// Wraps the given string value.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// Borrows the inner string.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes self and returns the inner string.
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
