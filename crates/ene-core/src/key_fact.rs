use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyFact {
    pub key: String,
    pub value: String,
}
