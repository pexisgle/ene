use serde::{Deserialize, Serialize};

use crate::request::AuthzRequest;

/// Rule decision from the policy DSL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Ask,
    Ai,
    Deny,
}

/// One first-match policy rule (P-903).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub decision: PolicyDecision,
}

impl PolicyRule {
    #[must_use]
    pub fn matches(&self, req: &AuthzRequest) -> bool {
        if !tool_matches(&self.tool, &req.tool) {
            return false;
        }
        match self.scope.as_deref() {
            Some("workspace") => req.in_workspace,
            _ => true,
        }
    }
}

/// On-disk policy document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyFile {
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl PolicyFile {
    #[must_use]
    pub fn first_match(&self, req: &AuthzRequest) -> Option<&PolicyRule> {
        self.rules.iter().find(|rule| rule.matches(req))
    }

    pub fn load_json(path: &std::path::Path) -> Result<Self, std::io::Error> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(std::io::Error::other)
    }

    pub fn save_json(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

fn tool_matches(pattern: &str, tool: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return tool == prefix || tool.starts_with(&format!("{prefix}."));
    }
    pattern == tool
}
