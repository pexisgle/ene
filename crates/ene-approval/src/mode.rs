use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `Inherit` is only meaningful in a per-plugin override; in the global
/// policy it resolves as `Ask`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Follow the global policy (per-plugin overrides only).
    Inherit,
    /// Always ask the user (interactive confirmation).
    #[default]
    Ask,
    /// Always allow, subject to mandatory security constraints.
    Allow,
    /// Always deny, even when the manifest declares the capability.
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedMode {
    /// Needs interactive confirmation; headless consumers fail safe to deny.
    Ask,
    /// Automatically allowed (still audited and still subject to mandatory
    /// security constraints).
    Allow,
    Deny,
}

impl ResolvedMode {
    #[must_use]
    pub const fn allows(self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        for mode in [
            ApprovalMode::Inherit,
            ApprovalMode::Ask,
            ApprovalMode::Allow,
            ApprovalMode::Deny,
        ] {
            let json = serde_json::to_value(mode).expect("serialize");
            let back: ApprovalMode = serde_json::from_value(json).expect("deserialize");
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn allows_reflects_allow_only() {
        assert!(ResolvedMode::Allow.allows());
        assert!(!ResolvedMode::Ask.allows());
        assert!(!ResolvedMode::Deny.allows());
    }
}
