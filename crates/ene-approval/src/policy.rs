use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::category::{ALL_CATEGORIES, ApprovalCategory};
use crate::mode::{ApprovalMode, ResolvedMode};

/// Global approval policy: one [`ApprovalMode`] per category.
///
/// Every category defaults to `Ask`. The global policy is the baseline;
/// per-plugin overrides ([`PluginApprovalPolicy`]) win when they do not say
/// `Inherit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ApprovalPolicy {
    /// Per-category default modes. Absent categories resolve as `Ask`.
    pub categories: BTreeMap<ApprovalCategory, ApprovalMode>,
    /// Emergency stop. When `true` every request resolves to `Deny`
    /// regardless of per-plugin and global settings, until it is cleared.
    pub emergency_stop: bool,
    /// Whether the two-step high-risk warning was confirmed for the current
    /// set of high-risk allowances. Cleared automatically whenever the
    /// settings UI changes a high-risk category, so a later change needs a
    /// fresh confirmation.
    pub high_risk_confirmed: bool,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            categories: ALL_CATEGORIES
                .iter()
                .map(|&category| (category, ApprovalMode::Ask))
                .collect(),
            emergency_stop: false,
            high_risk_confirmed: false,
        }
    }
}

impl ApprovalPolicy {
    /// The mode for `category`, defaulting to `Ask` when absent.
    #[must_use]
    pub fn mode(&self, category: ApprovalCategory) -> ApprovalMode {
        self.categories
            .get(&category)
            .copied()
            .unwrap_or(ApprovalMode::Ask)
    }

    /// Whether any high-risk category is set to `Allow` at the global level.
    #[must_use]
    pub fn has_high_risk_allow(&self) -> bool {
        self.categories
            .iter()
            .any(|(&category, &mode)| category.is_high_risk() && mode == ApprovalMode::Allow)
    }
}

/// Per-plugin override of the global policy.
///
/// `Inherit` (the default for every category) delegates to the global
/// policy; any other mode wins for this plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PluginApprovalPolicy {
    /// Per-category overrides. Absent categories inherit the global policy.
    pub categories: BTreeMap<ApprovalCategory, ApprovalMode>,
}

impl Default for PluginApprovalPolicy {
    fn default() -> Self {
        Self {
            categories: ALL_CATEGORIES
                .iter()
                .map(|&category| (category, ApprovalMode::Inherit))
                .collect(),
        }
    }
}

impl PluginApprovalPolicy {
    /// The override for `category` (`Inherit` when absent).
    #[must_use]
    pub fn mode(&self, category: ApprovalCategory) -> ApprovalMode {
        self.categories
            .get(&category)
            .copied()
            .unwrap_or(ApprovalMode::Inherit)
    }

    /// Whether this plugin override allows any high-risk category.
    #[must_use]
    pub fn has_high_risk_allow(&self) -> bool {
        self.categories
            .iter()
            .any(|(&category, &mode)| category.is_high_risk() && mode == ApprovalMode::Allow)
    }
}

/// Which rule produced a [`Resolution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionReason {
    /// The emergency stop forced a denial.
    EmergencyStop,
    /// The per-plugin override decided.
    PluginOverride,
    /// The global policy decided.
    GlobalPolicy,
    /// No policy entry exists; the fail-safe default applies.
    DefaultAsk,
}

impl ResolutionReason {
    /// Stable audit-log label for this reason.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::EmergencyStop => "emergency_stop",
            Self::PluginOverride => "plugin_override",
            Self::GlobalPolicy => "global_policy",
            Self::DefaultAsk => "default_ask",
        }
    }
}

/// The outcome of resolving one request, including the rule that matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    /// The effective mode.
    pub mode: ResolvedMode,
    /// Which layer decided.
    pub reason: ResolutionReason,
    /// Human-readable description of the applied rule (audit-friendly).
    pub rule: &'static str,
}

/// Resolves requests against global and per-plugin policies.
#[derive(Debug)]
pub struct ApprovalResolver<'a> {
    global: &'a ApprovalPolicy,
    plugins: &'a BTreeMap<String, PluginApprovalPolicy>,
}

impl<'a> ApprovalResolver<'a> {
    /// Builds a resolver over the given policies.
    #[must_use]
    pub fn new(
        global: &'a ApprovalPolicy,
        plugins: &'a BTreeMap<String, PluginApprovalPolicy>,
    ) -> Self {
        Self { global, plugins }
    }

    /// Resolves `category` for `plugin`.
    ///
    /// Order: emergency stop → per-plugin override → global policy → `Ask`.
    /// A resolution of `Ask` still requires a human answer at the call site;
    /// headless consumers must treat it as denied.
    #[must_use]
    pub fn resolve(&self, plugin: &str, category: ApprovalCategory) -> Resolution {
        if self.global.emergency_stop {
            return Resolution {
                mode: ResolvedMode::Deny,
                reason: ResolutionReason::EmergencyStop,
                rule: "emergency stop is active; all plugin requests are denied",
            };
        }
        if let Some(plugin_policy) = self.plugins.get(plugin) {
            match plugin_policy.mode(category) {
                ApprovalMode::Allow => {
                    return Resolution {
                        mode: ResolvedMode::Allow,
                        reason: ResolutionReason::PluginOverride,
                        rule: "per-plugin override allows this category",
                    };
                }
                ApprovalMode::Deny => {
                    return Resolution {
                        mode: ResolvedMode::Deny,
                        reason: ResolutionReason::PluginOverride,
                        rule: "per-plugin override denies this category",
                    };
                }
                ApprovalMode::Ask => {
                    return Resolution {
                        mode: ResolvedMode::Ask,
                        reason: ResolutionReason::PluginOverride,
                        rule: "per-plugin override requires confirmation",
                    };
                }
                ApprovalMode::Inherit => {}
            }
        }
        match self.global.mode(category) {
            ApprovalMode::Allow => Resolution {
                mode: ResolvedMode::Allow,
                reason: ResolutionReason::GlobalPolicy,
                rule: "global policy allows this category",
            },
            ApprovalMode::Deny => Resolution {
                mode: ResolvedMode::Deny,
                reason: ResolutionReason::GlobalPolicy,
                rule: "global policy denies this category",
            },
            ApprovalMode::Ask | ApprovalMode::Inherit => Resolution {
                mode: ResolvedMode::Ask,
                reason: ResolutionReason::GlobalPolicy,
                rule: "global policy requires confirmation",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policies(
        global: ApprovalPolicy,
        plugins: &[(&str, PluginApprovalPolicy)],
    ) -> (ApprovalPolicy, BTreeMap<String, PluginApprovalPolicy>) {
        (
            global,
            plugins
                .iter()
                .map(|(name, policy)| ((*name).to_string(), policy.clone()))
                .collect(),
        )
    }

    #[test]
    fn default_policy_is_ask_everywhere() {
        let policy = ApprovalPolicy::default();
        for category in ALL_CATEGORIES {
            assert_eq!(policy.mode(*category), ApprovalMode::Ask);
        }
        assert!(!policy.has_high_risk_allow());
    }

    #[test]
    fn plugin_override_wins_over_global() {
        let mut global = ApprovalPolicy::default();
        global
            .categories
            .insert(ApprovalCategory::FsRead, ApprovalMode::Deny);
        let mut plugin = PluginApprovalPolicy::default();
        plugin
            .categories
            .insert(ApprovalCategory::FsRead, ApprovalMode::Allow);
        let (global, plugins) = policies(global, &[("fs", plugin)]);
        let resolver = ApprovalResolver::new(&global, &plugins);
        let resolution = resolver.resolve("fs", ApprovalCategory::FsRead);
        assert_eq!(resolution.mode, ResolvedMode::Allow);
        assert_eq!(resolution.reason, ResolutionReason::PluginOverride);
        // Another plugin inherits the global denial.
        let resolution = resolver.resolve("other", ApprovalCategory::FsRead);
        assert_eq!(resolution.mode, ResolvedMode::Deny);
        assert_eq!(resolution.reason, ResolutionReason::GlobalPolicy);
    }

    #[test]
    fn inherit_delegates_to_global() {
        let mut global = ApprovalPolicy::default();
        global
            .categories
            .insert(ApprovalCategory::DynamicHttps, ApprovalMode::Allow);
        let plugin = PluginApprovalPolicy::default();
        let (global, plugins) = policies(global, &[("web", plugin)]);
        let resolver = ApprovalResolver::new(&global, &plugins);
        let resolution = resolver.resolve("web", ApprovalCategory::DynamicHttps);
        assert_eq!(resolution.mode, ResolvedMode::Allow);
        assert_eq!(resolution.reason, ResolutionReason::GlobalPolicy);
    }

    #[test]
    fn emergency_stop_beats_everything() {
        let mut global = ApprovalPolicy::default();
        global
            .categories
            .insert(ApprovalCategory::FsDelete, ApprovalMode::Allow);
        global.emergency_stop = true;
        let mut plugin = PluginApprovalPolicy::default();
        plugin
            .categories
            .insert(ApprovalCategory::FsDelete, ApprovalMode::Allow);
        let (global, plugins) = policies(global, &[("fs", plugin)]);
        let resolver = ApprovalResolver::new(&global, &plugins);
        for plugin_name in ["fs", "other"] {
            let resolution = resolver.resolve(plugin_name, ApprovalCategory::FsDelete);
            assert_eq!(resolution.mode, ResolvedMode::Deny);
            assert_eq!(resolution.reason, ResolutionReason::EmergencyStop);
        }
    }

    #[test]
    fn unknown_category_resolves_to_ask() {
        let global = ApprovalPolicy::default();
        let (global, plugins) = policies(global, &[]);
        let resolver = ApprovalResolver::new(&global, &plugins);
        let resolution = resolver.resolve("x", ApprovalCategory::Browser);
        assert_eq!(resolution.mode, ResolvedMode::Ask);
        assert_eq!(resolution.reason, ResolutionReason::GlobalPolicy);
    }

    #[test]
    fn high_risk_flag_tracks_allow_only() {
        let mut global = ApprovalPolicy::default();
        assert!(!global.has_high_risk_allow());
        global
            .categories
            .insert(ApprovalCategory::Shell, ApprovalMode::Allow);
        assert!(global.has_high_risk_allow());
        global
            .categories
            .insert(ApprovalCategory::Shell, ApprovalMode::Ask);
        assert!(!global.has_high_risk_allow());

        let mut plugin = PluginApprovalPolicy::default();
        assert!(!plugin.has_high_risk_allow());
        plugin
            .categories
            .insert(ApprovalCategory::Http, ApprovalMode::Allow);
        assert!(plugin.has_high_risk_allow());
    }
}
