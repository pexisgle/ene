use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Every operation category the approval system can gate.
///
/// The wire/serde form is `snake_case` so policies persist readably in
/// `settings.json`. New categories must be added to [`ALL_CATEGORIES`] so
/// policy defaults cover them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalCategory {
    /// Reading a user file through the `File` broker.
    FsRead,
    /// Creating a file or directory through the `File` broker.
    FsCreate,
    /// Modifying an existing file through the `File` broker.
    FsModify,
    /// Deleting a file or directory through the `File` broker.
    FsDelete,
    /// Talking to a manifest-declared fixed origin through the `Network` broker.
    FixedOriginNetwork,
    /// Dynamic HTTPS access (any public site) through the `Network` broker.
    DynamicHttps,
    /// Plain-HTTP access (separate approval; credentials are never injected).
    Http,
    /// LAN access (denied in v1; the category exists so policies can be
    /// written before the capability ships).
    Lan,
    /// Loopback access (always denied; the category exists for policy
    /// completeness and is unreachable in practice).
    Loopback,
    /// Saving a downloaded web file to a user folder.
    WebFileSave,
    /// Installing a plugin artifact.
    PluginInstall,
    /// Updating a plugin artifact.
    PluginUpdate,
    /// Installing a sidecar artifact.
    SidecarInstall,
    /// Updating a sidecar artifact.
    SidecarUpdate,
    /// Installing a model artifact.
    ModelInstall,
    /// Updating a model artifact.
    ModelUpdate,
    /// Spawning a child process through the `Process` broker.
    ProcessSpawn,
    /// Executing a shell command through the `Process` broker.
    Shell,
    /// Controlling a browser through the platform layer.
    Browser,
    /// Platform capabilities (open external URL, system info).
    Platform,
    /// Using a stored credential (the key name only ever reaches the audit log).
    CredentialUse,
}

/// Every category, in display order. Policy defaults iterate this list.
pub const ALL_CATEGORIES: &[ApprovalCategory] = &[
    ApprovalCategory::FsRead,
    ApprovalCategory::FsCreate,
    ApprovalCategory::FsModify,
    ApprovalCategory::FsDelete,
    ApprovalCategory::FixedOriginNetwork,
    ApprovalCategory::DynamicHttps,
    ApprovalCategory::Http,
    ApprovalCategory::Lan,
    ApprovalCategory::Loopback,
    ApprovalCategory::WebFileSave,
    ApprovalCategory::PluginInstall,
    ApprovalCategory::PluginUpdate,
    ApprovalCategory::SidecarInstall,
    ApprovalCategory::SidecarUpdate,
    ApprovalCategory::ModelInstall,
    ApprovalCategory::ModelUpdate,
    ApprovalCategory::ProcessSpawn,
    ApprovalCategory::Shell,
    ApprovalCategory::Browser,
    ApprovalCategory::Platform,
    ApprovalCategory::CredentialUse,
];

/// Categories whose automatic allowance requires the two-step high-risk
/// confirmation and a persistent warning in the settings UI.
pub const HIGH_RISK_CATEGORIES: &[ApprovalCategory] = &[
    ApprovalCategory::FsCreate,
    ApprovalCategory::FsModify,
    ApprovalCategory::FsDelete,
    ApprovalCategory::Http,
    ApprovalCategory::Lan,
    ApprovalCategory::Loopback,
    ApprovalCategory::PluginInstall,
    ApprovalCategory::PluginUpdate,
    ApprovalCategory::SidecarInstall,
    ApprovalCategory::SidecarUpdate,
    ApprovalCategory::ModelInstall,
    ApprovalCategory::ModelUpdate,
    ApprovalCategory::ProcessSpawn,
    ApprovalCategory::Shell,
    ApprovalCategory::Browser,
    ApprovalCategory::CredentialUse,
];

impl ApprovalCategory {
    /// Whether automatic allowance of this category is high-risk.
    #[must_use]
    pub const fn is_high_risk(self) -> bool {
        matches!(
            self,
            Self::FsCreate
                | Self::FsModify
                | Self::FsDelete
                | Self::Http
                | Self::Lan
                | Self::Loopback
                | Self::PluginInstall
                | Self::PluginUpdate
                | Self::SidecarInstall
                | Self::SidecarUpdate
                | Self::ModelInstall
                | Self::ModelUpdate
                | Self::ProcessSpawn
                | Self::Shell
                | Self::Browser
                | Self::CredentialUse
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_categories_cover_high_risk() {
        for category in HIGH_RISK_CATEGORIES {
            assert!(ALL_CATEGORIES.contains(category));
            assert!(category.is_high_risk());
        }
    }

    #[test]
    fn benign_categories_are_not_high_risk() {
        assert!(!ApprovalCategory::FsRead.is_high_risk());
        assert!(!ApprovalCategory::DynamicHttps.is_high_risk());
        assert!(!ApprovalCategory::FixedOriginNetwork.is_high_risk());
        assert!(!ApprovalCategory::WebFileSave.is_high_risk());
        assert!(!ApprovalCategory::Platform.is_high_risk());
    }

    #[test]
    fn serde_round_trip_uses_snake_case() {
        let json = serde_json::to_value(ApprovalCategory::DynamicHttps).expect("serialize");
        assert_eq!(json, serde_json::json!("dynamic_https"));
        let back: ApprovalCategory = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, ApprovalCategory::DynamicHttps);
    }
}
