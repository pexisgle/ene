use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::category::ApprovalCategory;

/// One permission request awaiting (or that received) a resolution.
///
/// `target` is a display/audit-safe description of what is being accessed
/// (an origin, a canonical path, an artifact id+version, a process command
/// line, a credential key name). It never carries file contents, request
/// bodies, or secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    /// Unique request id (UUID), used to correlate a later answer.
    pub id: String,
    /// Plugin name requesting the capability.
    pub plugin: String,
    /// Digest of the plugin's signed manifest, when one is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    /// Category being requested.
    pub category: ApprovalCategory,
    /// Audit-safe target description.
    pub target: String,
    /// Optional human-readable detail shown in the confirmation UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Whether this request is high-risk (mirrors
    /// [`ApprovalCategory::is_high_risk`] at request time).
    pub high_risk: bool,
}

impl ApprovalRequest {
    /// Builds a request with a fresh UUID.
    #[must_use]
    pub fn new(
        plugin: String,
        manifest_digest: Option<String>,
        category: ApprovalCategory,
        target: String,
        detail: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            plugin,
            manifest_digest,
            category,
            target,
            detail,
            high_risk: category.is_high_risk(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_marks_high_risk_from_category() {
        let request = ApprovalRequest::new(
            "fs".to_string(),
            Some("abc".to_string()),
            ApprovalCategory::Shell,
            "rm -rf /".to_string(),
            None,
        );
        assert!(request.high_risk);
        assert_eq!(request.plugin, "fs");
        assert!(uuid::Uuid::parse_str(&request.id).is_ok());

        let request = ApprovalRequest::new(
            "web".to_string(),
            None,
            ApprovalCategory::DynamicHttps,
            "https://example.com".to_string(),
            None,
        );
        assert!(!request.high_risk);
    }
}
