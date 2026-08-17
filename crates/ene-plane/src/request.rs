use serde::{Deserialize, Serialize};

/// Host-only sensitivity. Empty `side_effects` can still require approval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    #[default]
    None,
    Medium,
    High,
}

/// What the plane evaluates. Host-only; never sent to the model.
#[derive(Debug, Clone)]
pub struct AuthzRequest {
    pub tool: String,
    pub side_effects: Vec<String>,
    pub sensitivity: Sensitivity,
    pub target: String,
    pub in_workspace: bool,
}
