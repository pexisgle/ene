//! Public plugin settings snapshot for the plugin center UI.
//!
//! The desktop renders one card per entry from [`PluginHostManager::settings_snapshots`];
//! MCP servers are pseudo-plugins the desktop assembles itself from the
//! `plugins.mcp_servers` section.

use crate::config::FsGrantConfig;
use ene_approval::{ApprovalCategory, ApprovalMode};
use ene_connector::declaration::CredentialDeclaration;
use ene_plugin_proto::PluginCapabilities;
use std::collections::BTreeMap;

/// Derived health state of a plugin for the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginHealthState {
    /// Enabled and connected to a supervised process.
    Running,
    /// Permanently disabled by the supervisor (restart budget / checksum).
    Disabled,
    /// Never started because hard capability requirements have no provider.
    RequirementsUnmet,
    /// Disabled in config, or enabled but not supervised (startup gate).
    Stopped,
}

impl PluginHealthState {
    /// Stable English code contract for diagnostics and i18n lookup.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Disabled => "disabled",
            Self::RequirementsUnmet => "requirements_unmet",
            Self::Stopped => "stopped",
        }
    }
}

/// Compact manifest facts shown in the plugin detail pane.
#[derive(Debug, Clone, Default)]
pub struct PluginManifestSummary {
    /// Publisher key id that signed the manifest, if any.
    pub key_id: Option<String>,
    /// Whether the manifest carried a signature at all.
    pub signed: bool,
    /// Configured binary checksum (user-pinned), if any.
    pub checksum: Option<String>,
}

/// One tool/action the plugin exposes, for search and dedicated sections.
#[derive(Debug, Clone)]
pub struct ToolActionInfo {
    /// Fully-qualified tool name (e.g. `fs.read`).
    pub name: String,
    /// Description shown to the LLM / search index.
    pub description: String,
}

/// Effective security posture of one plugin entry, resolved from the global
/// plugin policy and the entry's own overrides.
#[derive(Debug, Clone, Default)]
pub struct EffectiveSecurity {
    /// Resolved approval mode per category, keyed by category code.
    pub approvals: BTreeMap<String, String>,
    /// Whether the global emergency stop is active.
    pub emergency_stop: bool,
    /// User-approved filesystem grants (logical slot → real path).
    pub fs_grants: Vec<FsGrantConfig>,
    /// Whether the OS sandbox applies to this plugin.
    pub sandbox_enabled: bool,
    /// DB quota in MiB (`None` = unbounded).
    pub db_quota_mb: Option<u64>,
}

/// Settings-relevant snapshot of one plugin.
///
/// `schema` is the plugin's JSON Schema plus `x-ene-*` extensions;
/// `config` / `profiles` are the values stored under `plugins.list.<name>`.
#[derive(Debug, Clone)]
pub struct PluginSettingsSnapshot {
    /// Plugin name (the `plugins.list` key).
    pub id: String,
    /// Capability-derived kind: `"tool"`, `"provider"`, `"hybrid"`, or
    /// `"unknown"`.
    pub kind: String,
    /// Whether the entry is enabled in config.
    pub enabled: bool,
    /// Derived health state.
    pub health: PluginHealthState,
    /// Capabilities advertised at the handshake.
    pub capabilities: PluginCapabilities,
    /// Tool/action names and descriptions advertised by the plugin, empty
    /// for provider-only plugins.
    pub actions: Vec<ToolActionInfo>,
    /// Manifest facts (signature / checksum).
    pub manifest: PluginManifestSummary,
    /// Latest config schema, `None` when the plugin advertises none.
    pub schema: Option<serde_json::Value>,
    /// Schema version the plugin expects (`0` = unversioned).
    pub schema_version: u32,
    /// Whether the plugin answers `ListConfigOptions` (dynamic options).
    pub supports_dynamic_config: bool,
    /// Whether the plugin answers `ValidateConfig` (delegated validation).
    pub supports_validate_config: bool,
    /// Current config blob delivered to the plugin.
    pub config: Option<serde_json::Value>,
    /// Per-profile config blobs, keyed by profile name.
    pub profiles: Option<serde_json::Value>,
    /// Credential declarations parsed from `x-ene-credentials`.
    pub credentials: Vec<CredentialDeclaration>,
    /// Effective approval / sandbox / quota posture.
    pub effective_security: EffectiveSecurity,
}

impl PluginSettingsSnapshot {
    /// Classifies the plugin kind from its advertised capabilities.
    #[must_use]
    pub fn classify_kind(capabilities: &PluginCapabilities) -> &'static str {
        let provides_provider = !capabilities.llm_providers.is_empty()
            || !capabilities.embed_providers.is_empty()
            || !capabilities.tts_providers.is_empty()
            || !capabilities.stt_providers.is_empty()
            || !capabilities.vad_providers.is_empty();
        let provides_tools = capabilities.tools > 0;
        match (provides_tools, provides_provider) {
            (true, true) => "hybrid",
            (true, false) => "tool",
            (false, true) => "provider",
            (false, false) => "unknown",
        }
    }

    /// Resolves the effective approval mode for `category`, applying the
    /// plugin override when it does not say `Inherit`.
    #[must_use]
    pub fn resolved_approval_mode(
        global: &BTreeMap<ApprovalCategory, ApprovalMode>,
        plugin_override: Option<&BTreeMap<ApprovalCategory, ApprovalMode>>,
        category: ApprovalCategory,
    ) -> ApprovalMode {
        plugin_override
            .and_then(|overrides| overrides.get(&category).copied())
            .filter(|mode| *mode != ApprovalMode::Inherit)
            .or_else(|| global.get(&category).copied())
            .unwrap_or(ApprovalMode::Ask)
    }
}

/// Serializes a `snake_case` serde enum to its code string.
///
/// Used for category / mode codes that persist readably in `settings.json`;
/// falls back to an empty string when the value cannot serialize to a string.
#[must_use]
pub fn serde_code<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_approval::{ApprovalCategory, ApprovalMode};
    use ene_plugin_proto::{
        ConcurrencyHint, LlmProviderSpec, PluginCapabilities, ResourceClass, TtsProviderSpec,
        VadProviderSpec,
    };

    #[test]
    fn kind_classification_covers_all_capability_shapes() {
        let tool = PluginCapabilities {
            tools: 3,
            ..PluginCapabilities::default()
        };
        assert_eq!(PluginSettingsSnapshot::classify_kind(&tool), "tool");

        let mut provider = PluginCapabilities::default();
        provider.llm_providers.push(LlmProviderSpec {
            kind: "openai".to_string(),
            supported_models: vec!["gpt-4o".to_string()],
            supports_streaming: true,
            supports_vision: false,
            concurrency: ConcurrencyHint::default(),
            context_window: None,
            resource_class: ResourceClass::Cpu,
        });
        assert_eq!(PluginSettingsSnapshot::classify_kind(&provider), "provider");

        provider.tools = 1;
        assert_eq!(PluginSettingsSnapshot::classify_kind(&provider), "hybrid");

        let mut voice = PluginCapabilities::default();
        voice.tts_providers.push(TtsProviderSpec {
            kind: "kokoro".to_string(),
            voices: vec!["af_heart".to_string()],
            formats: Vec::new(),
            concurrency: ConcurrencyHint::default(),
        });
        voice.vad_providers.push(VadProviderSpec {
            kind: "silero".to_string(),
            frame_size: 512,
            sample_rate: 16_000,
            concurrency: ConcurrencyHint::default(),
        });
        assert_eq!(PluginSettingsSnapshot::classify_kind(&voice), "provider");

        assert_eq!(
            PluginSettingsSnapshot::classify_kind(&PluginCapabilities::default()),
            "unknown"
        );
    }

    #[test]
    fn approval_resolution_applies_overrides_then_global_then_ask() {
        let mut global = BTreeMap::new();
        global.insert(ApprovalCategory::FsRead, ApprovalMode::Allow);
        let mut overrides = BTreeMap::new();
        overrides.insert(ApprovalCategory::FsRead, ApprovalMode::Inherit);
        overrides.insert(ApprovalCategory::FsDelete, ApprovalMode::Deny);

        assert_eq!(
            PluginSettingsSnapshot::resolved_approval_mode(
                &global,
                Some(&overrides),
                ApprovalCategory::FsRead
            ),
            ApprovalMode::Allow,
            "Inherit delegates to the global policy"
        );
        assert_eq!(
            PluginSettingsSnapshot::resolved_approval_mode(
                &global,
                Some(&overrides),
                ApprovalCategory::FsDelete
            ),
            ApprovalMode::Deny,
            "a concrete override wins"
        );
        assert_eq!(
            PluginSettingsSnapshot::resolved_approval_mode(
                &global,
                Some(&overrides),
                ApprovalCategory::Http
            ),
            ApprovalMode::Ask,
            "absent everywhere falls back to Ask"
        );
        assert_eq!(
            PluginSettingsSnapshot::resolved_approval_mode(&global, None, ApprovalCategory::FsRead),
            ApprovalMode::Allow
        );
    }

    #[test]
    fn serde_code_emits_snake_case_strings() {
        assert_eq!(serde_code(&ApprovalMode::Inherit), "inherit");
        assert_eq!(serde_code(&ApprovalMode::Allow), "allow");
        assert_eq!(serde_code(&ApprovalCategory::DynamicHttps), "dynamic_https");
    }

    #[test]
    fn health_state_codes_are_stable() {
        assert_eq!(PluginHealthState::Running.code(), "running");
        assert_eq!(PluginHealthState::Disabled.code(), "disabled");
        assert_eq!(
            PluginHealthState::RequirementsUnmet.code(),
            "requirements_unmet"
        );
        assert_eq!(PluginHealthState::Stopped.code(), "stopped");
    }
}
