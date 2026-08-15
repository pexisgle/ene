//! Unified settings-apply contract.
//!
//! Replaces the split `UpdateProactiveSettings` / `UpdateFeatureSettings`
//! commands: the UI drafts a full config, tags it with a monotonic revision,
//! and the actor diffs it against its live config, applies the changed
//! sections, and reports the actual impact plus any per-section failures.
//! The echoed [`SettingsApplyResult::revision`] lets the UI detect a stale
//! apply (a newer draft won the race) and re-sync instead of silently losing
//! edits.

use ene_config::EneConfig;
use std::collections::BTreeSet;

/// How an applied change affects the running process.
///
/// The desktop displays this on apply results and per-field metadata so a
/// user knows whether a change took effect immediately, hot-reloads a
/// subsystem, or requires a plugin / app restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SettingsImpact {
    /// Hot-reloadable sections changed (`mind` / `store` / `ai` / `rag` /
    /// top-level prompt fields); the actor re-applied them live.
    pub runtime_reload: bool,
    /// The enabled plugin set changed; the plugin host restarts and the tool
    /// registry is rebuilt.
    pub plugin_restart: bool,
    /// The change cannot take effect until the whole app restarts.
    pub app_restart: bool,
}

impl SettingsImpact {
    /// True when no restart or hot-reload was needed.
    #[must_use]
    pub const fn immediate(self) -> bool {
        !self.runtime_reload && !self.plugin_restart && !self.app_restart
    }
}

/// A settings apply proposed by the UI.
///
/// `config` is the full merged draft; the actor diffs it against its own
/// config so only genuinely changed sections are written and reacted to.
#[derive(Clone)]
pub struct SettingsApplyRequest {
    /// Monotonic revision assigned by the drafting UI. Echoed back verbatim
    /// in the result for stale-apply detection.
    pub revision: u64,
    /// The actor-side settings revision this draft was based on. The actor
    /// rejects the apply with [`SettingsApplyResult::conflicted`] when its
    /// own revision has moved past this value (another writer won), so the
    /// UI re-syncs instead of silently overwriting newer state. `None`
    /// bypasses the check (initial sync / one-shot callers).
    pub base_revision: Option<u64>,
    pub config: EneConfig,
}

/// `Debug` never prints the merged config: it holds real secret values.
impl std::fmt::Debug for SettingsApplyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsApplyRequest")
            .field("revision", &self.revision)
            .field("base_revision", &self.base_revision)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct SettingsApplyResult {
    /// The request revision this result answers.
    pub revision: u64,
    /// The actor's settings revision after this attempt.
    pub current_revision: u64,
    /// True when `base_revision` did not match the actor's revision: nothing
    /// was applied and the UI must re-sync its draft before retrying.
    pub conflicted: bool,
    /// Section keys (plus top-level prompt fields) actually written.
    pub applied_sections: BTreeSet<String>,
    /// What the apply required beyond writing config.
    pub impact: SettingsImpact,
    /// Per-section failures, if any. Hard errors (channel / config write)
    /// surface as `Err` on the apply call instead; the actor rolls the config
    /// back before reporting those.
    pub errors: Vec<String>,
}

/// Top-level [`EneConfig`] fields the actor mirrors into its live config on
/// apply, alongside the `extra` sections.
const DECLARED_TOP_LEVEL_FIELDS: &[&str] =
    &["character", "user_name", "runtime_rules", "user_persona"];

/// Section keys (and top-level prompt fields) whose value differs between two
/// configs.
///
/// Compares the union of `extra` keys and the declared top-level prompt
/// fields. Order is deterministic (sorted) so callers and tests can rely on
/// stable output.
#[must_use]
pub fn changed_sections(prev: &EneConfig, next: &EneConfig) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for key in prev.extra.keys().chain(next.extra.keys()) {
        keys.insert(key.clone());
    }
    for field in DECLARED_TOP_LEVEL_FIELDS {
        keys.insert((*field).to_string());
    }
    keys.into_iter()
        .filter(|key| {
            top_level_changed(prev, next, key) || prev.extra.get(key) != next.extra.get(key)
        })
        .collect()
}

fn top_level_changed(prev: &EneConfig, next: &EneConfig, key: &str) -> bool {
    match key {
        "character" => prev.character != next.character,
        "user_name" => prev.user_name != next.user_name,
        "runtime_rules" => prev.runtime_rules != next.runtime_rules,
        "user_persona" => prev.user_persona != next.user_persona,
        _ => false,
    }
}
