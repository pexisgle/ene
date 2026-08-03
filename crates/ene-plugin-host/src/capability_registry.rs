//! Host-side registry of plugin capability declarations and requirement
//! resolution.
//!
//! [`CapabilityRegistry`] indexes the `provides` / `requires` declarations
//! plugins advertise during the handshake and resolves a `requires` entry to
//! the plugin that provides it. The registry is pure — no I/O, no plugin
//! processes — so it is unit-testable without plugin infrastructure, and it
//! is the future ACL source for host-mediated capability calls.

use std::collections::{BTreeMap, BTreeSet};

use ene_plugin_proto::{CapabilityRef, CapabilityRequirement, PluginCapabilities};

/// A validated capability identity used as the registry index key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CapabilityId {
    name: String,
    major: u32,
}

impl CapabilityId {
    fn from_ref(capability: &CapabilityRef) -> Option<Self> {
        let capability = CapabilityRef::parse(capability.as_str()).ok()?;
        Some(Self {
            name: capability.name()?.to_string(),
            major: capability.major()?,
        })
    }

    fn from_requirement(requirement: &CapabilityRequirement) -> Option<Self> {
        let requirement = CapabilityRequirement::parse(requirement.as_str()).ok()?;
        Some(Self {
            name: requirement.name()?.to_string(),
            major: requirement.major()?,
        })
    }
}

/// The validated capability declarations one plugin made during the
/// handshake.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginCapabilityDeclarations {
    /// Validated capabilities this plugin provides.
    pub provides: Vec<CapabilityRef>,
    /// Validated capabilities this plugin requires.
    pub requires: Vec<CapabilityRequirement>,
}

/// Index of capability declarations from all registered plugins.
///
/// Providers are keyed by capability identity; each identity maps to the set
/// of plugin names providing it. Resolution is deterministic: when several
/// plugins provide the same capability, the lexicographically smallest plugin
/// name wins. Plugin-config iteration order is not a valid tie-breaker
/// (`plugins.list` is a map), so no other precedence rule exists.
///
/// A plugin's requirement is satisfied by its own `provides` — self-resolution
/// is allowed. The future capability-mediation ACL decides separately whether
/// a plugin may *call* its own capability.
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    providers: BTreeMap<CapabilityId, BTreeSet<String>>,
    declarations: BTreeMap<String, PluginCapabilityDeclarations>,
}

impl CapabilityRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one plugin's handshake declarations, replacing any previous
    /// entry for the plugin.
    ///
    /// Invalid entries are warned about individually and dropped — one bad
    /// declaration never fails the plugin's handshake or affects the rest of
    /// its declarations (same per-entry policy as credential declarations).
    pub fn register(&mut self, plugin: &str, capabilities: &PluginCapabilities) {
        self.remove_provider(plugin);
        let mut declarations = PluginCapabilityDeclarations::default();
        for provided in &capabilities.provides {
            let Some(id) = CapabilityId::from_ref(provided) else {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin,
                    capability = %provided,
                    "Dropping malformed capability declaration"
                );
                continue;
            };
            self.providers
                .entry(id)
                .or_default()
                .insert(plugin.to_string());
            if !declarations.provides.contains(provided) {
                declarations.provides.push(provided.clone());
            }
        }
        for required in &capabilities.requires {
            if CapabilityId::from_requirement(required).is_none() {
                tracing::warn!(
                    component = "PluginHostManager",
                    plugin,
                    requirement = %required,
                    "Dropping malformed capability requirement"
                );
                continue;
            }
            if !declarations.requires.contains(required) {
                declarations.requires.push(required.clone());
            }
        }
        self.declarations.insert(plugin.to_string(), declarations);
    }

    /// Removes every capability `plugin` provides from the provider index.
    ///
    /// Used by the startup gate so a plugin disabled for unmet requirements
    /// does not keep satisfying other plugins' requirements.
    pub fn remove_provider(&mut self, plugin: &str) {
        self.providers.retain(|_, providers| {
            providers.remove(plugin);
            !providers.is_empty()
        });
    }

    /// Resolves `requirement` to the plugin name that provides it, if any.
    ///
    /// `None` for an unparsable requirement or when no provider is
    /// registered; soft requirements are resolved identically to hard ones —
    /// softness only changes what the caller does with `None`.
    #[must_use]
    pub fn resolve(&self, requirement: &CapabilityRequirement) -> Option<&str> {
        let id = CapabilityId::from_requirement(requirement)?;
        self.providers
            .get(&id)
            .and_then(|providers| providers.first())
            .map(String::as_str)
    }

    /// Returns the plugin names providing `name@major`, in deterministic
    /// (sorted) order.
    #[must_use]
    pub fn providers(&self, name: &str, major: u32) -> Vec<&str> {
        self.providers
            .get(&CapabilityId {
                name: name.to_string(),
                major,
            })
            .map_or_else(Vec::new, |providers| {
                providers.iter().map(String::as_str).collect()
            })
    }

    /// Returns the declarations registered for `plugin`.
    #[must_use]
    pub fn declarations(&self, plugin: &str) -> Option<&PluginCapabilityDeclarations> {
        self.declarations.get(plugin)
    }

    /// Returns `plugin`'s hard requirements with no provider, in declaration
    /// order. Soft requirements are never included.
    #[must_use]
    pub fn unmet_hard_requirements(&self, plugin: &str) -> Vec<&CapabilityRequirement> {
        self.unmet_requirements(plugin, false)
    }

    /// Returns `plugin`'s soft requirements with no provider, in declaration
    /// order.
    #[must_use]
    pub fn unmet_soft_requirements(&self, plugin: &str) -> Vec<&CapabilityRequirement> {
        self.unmet_requirements(plugin, true)
    }

    fn unmet_requirements(&self, plugin: &str, soft: bool) -> Vec<&CapabilityRequirement> {
        let Some(declarations) = self.declarations.get(plugin) else {
            return Vec::new();
        };
        declarations
            .requires
            .iter()
            .filter(|requirement| {
                requirement.is_soft() == soft && self.resolve(requirement).is_none()
            })
            .collect()
    }
}

/// The handshake declarations collected from one connected plugin.
pub struct CapabilityDeclaration {
    /// Plugin name.
    pub plugin: String,
    /// The capabilities the plugin advertised during the handshake.
    pub capabilities: PluginCapabilities,
}

/// Builds the registry from every startup declaration and returns the plugins
/// whose hard requirements are unmet, computed to a fixpoint.
///
/// A plugin disabled for unmet requirements must not count as a provider:
/// otherwise its consumers would resolve against a plugin that never becomes
/// available. The gate therefore removes disabled providers and re-evaluates
/// until no new plugin is disabled, so a chain of dependent plugins collapses
/// correctly. The returned disabled list is sorted for deterministic
/// diagnostics.
#[must_use]
pub fn evaluate_capability_gate(
    declarations: &[CapabilityDeclaration],
) -> (CapabilityRegistry, Vec<String>) {
    let mut registry = CapabilityRegistry::new();
    for declaration in declarations {
        registry.register(&declaration.plugin, &declaration.capabilities);
    }

    let mut disabled: BTreeSet<String> = BTreeSet::new();
    loop {
        let newly_disabled: Vec<String> = declarations
            .iter()
            .filter(|declaration| !disabled.contains(&declaration.plugin))
            .filter(|declaration| {
                !registry
                    .unmet_hard_requirements(&declaration.plugin)
                    .is_empty()
            })
            .map(|declaration| declaration.plugin.clone())
            .collect();
        if newly_disabled.is_empty() {
            break;
        }
        for plugin in &newly_disabled {
            registry.remove_provider(plugin);
            disabled.insert(plugin.clone());
        }
    }
    (registry, disabled.into_iter().collect())
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
mod tests {
    use super::*;

    fn caps(provides: &[&str], requires: &[&str]) -> PluginCapabilities {
        PluginCapabilities {
            provides: provides
                .iter()
                .map(|raw| CapabilityRef::parse(raw).unwrap())
                .collect(),
            requires: requires
                .iter()
                .map(|raw| CapabilityRequirement::parse(raw).unwrap())
                .collect(),
            ..PluginCapabilities::default()
        }
    }

    fn declaration(plugin: &str, provides: &[&str], requires: &[&str]) -> CapabilityDeclaration {
        CapabilityDeclaration {
            plugin: plugin.to_string(),
            capabilities: caps(provides, requires),
        }
    }

    fn requirement(raw: &str) -> CapabilityRequirement {
        CapabilityRequirement::parse(raw).unwrap()
    }

    #[test]
    fn resolve_exact_and_compatible_major() {
        let mut registry = CapabilityRegistry::new();
        registry.register(
            "local-llm",
            &caps(&["llm/chat@1", "embed@1", "gguf-runner@1"], &[]),
        );

        assert_eq!(
            registry.resolve(&requirement("gguf-runner@1")),
            Some("local-llm")
        );
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("local-llm")
        );
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1?")),
            Some("local-llm")
        );
        assert_eq!(
            registry.resolve(&requirement("llm/chat@1")),
            Some("local-llm")
        );
        assert_eq!(registry.resolve(&requirement("embed@1")), Some("local-llm"));
    }

    #[test]
    fn resolve_rejects_major_mismatch_and_unknown_name() {
        let mut registry = CapabilityRegistry::new();
        registry.register("local-llm", &caps(&["gguf-runner@1"], &[]));

        assert_eq!(registry.resolve(&requirement("gguf-runner@2")), None);
        assert_eq!(registry.resolve(&requirement("gguf-runner@^2")), None);
        assert_eq!(registry.resolve(&requirement("onnx-runner@1")), None);
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1?")),
            Some("local-llm")
        );
    }

    #[test]
    fn resolution_is_deterministic_across_competing_providers() {
        let mut registry = CapabilityRegistry::new();
        registry.register("z-provider", &caps(&["gguf-runner@1"], &[]));
        registry.register("a-provider", &caps(&["gguf-runner@1"], &[]));
        registry.register("m-provider", &caps(&["gguf-runner@1"], &[]));

        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("a-provider")
        );
        assert_eq!(
            registry.providers("gguf-runner", 1),
            ["a-provider", "m-provider", "z-provider"]
        );
    }

    #[test]
    fn distinct_majors_coexist() {
        let mut registry = CapabilityRegistry::new();
        registry.register("v1", &caps(&["gguf-runner@1"], &[]));
        registry.register("v2", &caps(&["gguf-runner@2"], &[]));

        assert_eq!(registry.resolve(&requirement("gguf-runner@1")), Some("v1"));
        assert_eq!(registry.resolve(&requirement("gguf-runner@2")), Some("v2"));
        assert_eq!(registry.resolve(&requirement("gguf-runner@^1")), Some("v1"));
    }

    #[test]
    fn self_provided_capability_satisfies_own_requirement() {
        let mut registry = CapabilityRegistry::new();
        registry.register("local-llm", &caps(&["gguf-runner@1"], &["gguf-runner@^1"]));

        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("local-llm")
        );
        assert!(registry.unmet_hard_requirements("local-llm").is_empty());
    }

    #[test]
    fn malformed_entries_are_dropped_individually() {
        let mut registry = CapabilityRegistry::new();
        let malformed: PluginCapabilities = serde_json::from_value(serde_json::json!({
            "provides": ["gguf-runner@1", "bad_ref"],
            "requires": ["gguf-runner@^1", "also_bad"]
        }))
        .unwrap();
        registry.register("local-llm", &malformed);

        let declarations = registry.declarations("local-llm").unwrap();
        assert_eq!(declarations.provides.len(), 1);
        assert_eq!(declarations.requires.len(), 1);
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("local-llm")
        );
    }

    #[test]
    fn malformed_names_are_dropped_from_wire_declarations() {
        let mut registry = CapabilityRegistry::new();
        let malformed: PluginCapabilities = serde_json::from_value(serde_json::json!({
            "provides": ["BAD@1", "bad_name@1", "gguf-runner@1"],
            "requires": ["/runner@^1", "gguf-runner@^1"]
        }))
        .unwrap();

        registry.register("local-llm", &malformed);

        let declarations = registry.declarations("local-llm").unwrap();
        assert_eq!(declarations.provides.len(), 1);
        assert_eq!(declarations.requires.len(), 1);
    }

    #[test]
    fn re_registering_replaces_provider_index_entries() {
        let mut registry = CapabilityRegistry::new();
        registry.register("provider", &caps(&["old@1"], &[]));
        registry.register("provider", &caps(&["new@1"], &[]));

        assert!(registry.providers("old", 1).is_empty());
        assert_eq!(registry.providers("new", 1), vec!["provider"]);
    }

    #[test]
    fn unmet_hard_requirements_excludes_soft() {
        let mut registry = CapabilityRegistry::new();
        registry.register("consumer", &caps(&[], &["gguf-runner@^1", "g2p/ja@^1?"]));

        assert_eq!(
            registry.unmet_hard_requirements("consumer"),
            vec![&requirement("gguf-runner@^1")]
        );
        assert_eq!(
            registry.unmet_soft_requirements("consumer"),
            vec![&requirement("g2p/ja@^1?")]
        );
    }

    #[test]
    fn unknown_plugin_has_no_unmet_requirements() {
        let registry = CapabilityRegistry::new();
        assert!(registry.unmet_hard_requirements("ghost").is_empty());
        assert!(registry.unmet_soft_requirements("ghost").is_empty());
        assert_eq!(registry.resolve(&requirement("gguf-runner@^1")), None);
    }

    #[test]
    fn gate_satisfied_requirement_does_not_disable_consumer() {
        let declarations = [
            declaration("local-llm", &["gguf-runner@1"], &[]),
            declaration("consumer", &[], &["gguf-runner@^1"]),
        ];

        let (registry, disabled) = evaluate_capability_gate(&declarations);
        assert!(disabled.is_empty());
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("local-llm")
        );
    }

    #[test]
    fn gate_disables_consumer_with_unmet_hard_requirement() {
        let declarations = [
            declaration("local-llm", &["gguf-runner@1"], &[]),
            declaration("consumer", &[], &["gguf-runner@^2"]),
        ];

        let (_, disabled) = evaluate_capability_gate(&declarations);
        assert_eq!(disabled, ["consumer"]);
    }

    #[test]
    fn gate_soft_requirement_falls_back_without_disabling() {
        let declarations = [
            declaration("local-llm", &["gguf-runner@1"], &[]),
            declaration("consumer", &[], &["gguf-runner@^2?"]),
        ];

        let (registry, disabled) = evaluate_capability_gate(&declarations);
        assert!(disabled.is_empty());
        assert_eq!(
            registry.unmet_soft_requirements("consumer"),
            vec![&requirement("gguf-runner@^2?")]
        );
    }

    #[test]
    fn gate_is_transitive_across_provider_chain() {
        let declarations = [
            declaration("runner", &["gguf-runner@1"], &["missing@1"]),
            declaration("consumer", &[], &["gguf-runner@^1"]),
        ];

        let (_, disabled) = evaluate_capability_gate(&declarations);
        // `runner` is disabled for its own unmet requirement, so it must not
        // satisfy `consumer` — the consumer is disabled too.
        assert_eq!(disabled, ["consumer", "runner"]);
    }

    #[test]
    fn gate_resolves_to_remaining_provider_after_winner_disabled() {
        let declarations = [
            declaration("a-provider", &["gguf-runner@1"], &["missing@1"]),
            declaration("z-provider", &["gguf-runner@1"], &[]),
            declaration("consumer", &[], &["gguf-runner@^1"]),
        ];

        let (registry, disabled) = evaluate_capability_gate(&declarations);
        // `a-provider` (lexicographic winner) is disabled; the consumer must
        // resolve to the remaining provider instead of being disabled.
        assert_eq!(disabled, ["a-provider"]);
        assert_eq!(
            registry.resolve(&requirement("gguf-runner@^1")),
            Some("z-provider")
        );
    }

    #[test]
    fn gate_duplicate_declarations_are_deduplicated() {
        let mut registry = CapabilityRegistry::new();
        registry.register(
            "local-llm",
            &caps(
                &["gguf-runner@1", "gguf-runner@1"],
                &["gguf-runner@^1", "gguf-runner@^1"],
            ),
        );

        let declarations = registry.declarations("local-llm").unwrap();
        assert_eq!(declarations.provides.len(), 1);
        assert_eq!(declarations.requires.len(), 1);
    }
}
