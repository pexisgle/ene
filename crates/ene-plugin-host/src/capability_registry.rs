//! Host-side registry of per-plugin capability declarations.
//!
//! [`CapabilityRegistry`] maps plugin names to the `provides` / `requires`
//! declarations parsed from each plugin's `x-ene-capabilities` schema block at
//! startup. [`resolve`](Self::resolve) answers "which plugin provides this
//! capability" for the runtime-sharing passenger slice, and
//! [`unmet_requires`](Self::unmet_requires) feeds the startup gate that
//! disables plugins whose hard requirements no running plugin satisfies.

use std::collections::HashMap;

use ene_connector::capability::{CapabilityId, ProvidedCapability, RequiredCapability};
use ene_connector::declaration::{
    CapabilityRejection, RejectedCapability, parse_capabilities, resolve_capability,
};
use parking_lot::RwLock;
use semver::VersionReq;
use serde_json::Value;

/// Registered capability declarations keyed by plugin name.
#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    provides: RwLock<HashMap<String, Vec<ProvidedCapability>>>,
    requires: RwLock<HashMap<String, Vec<RequiredCapability>>>,
}

impl CapabilityRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `schema`'s `x-ene-capabilities` block and records the result for
    /// `plugin`, replacing any previous entry.
    ///
    /// Rejected entries are warned about individually and dropped — one bad
    /// declaration never affects the rest, and the plugin itself is never
    /// involved. A `None` schema clears any previous entry for `plugin`.
    pub fn register_from_schema(&self, plugin: &str, schema: Option<&Value>) {
        let Some(schema) = schema else {
            self.register(plugin, Vec::new(), Vec::new());
            return;
        };
        let parse = parse_capabilities(schema);
        for rejected in &parse.rejected {
            warn_rejected_capability(plugin, rejected);
        }
        self.register(plugin, parse.provides, parse.requires);
    }

    /// Records `provides` and `requires` for `plugin`, replacing any previous
    /// entry.
    ///
    /// Idempotent: re-registering the same plugin (e.g. after a schema
    /// re-parse) simply overwrites.
    pub fn register(
        &self,
        plugin: &str,
        provides: Vec<ProvidedCapability>,
        requires: Vec<RequiredCapability>,
    ) {
        self.provides.write().insert(plugin.to_string(), provides);
        self.requires.write().insert(plugin.to_string(), requires);
    }

    /// All `(plugin, capability)` pairs that provide `name`, in arbitrary
    /// order.
    #[must_use]
    pub fn providers(&self, name: &CapabilityId) -> Vec<(String, ProvidedCapability)> {
        self.provides
            .read()
            .iter()
            .flat_map(|(plugin, caps)| {
                caps.iter()
                    .filter(|cap| &cap.name == name)
                    .map(move |cap| (plugin.clone(), cap.clone()))
            })
            .collect()
    }

    /// Resolves `req` for `name` to one provider's plugin name, choosing the
    /// lexicographically smallest matching plugin so the answer is
    /// deterministic regardless of `plugins.list` iteration order. Returns
    /// `None` when no registered provider satisfies the request.
    #[must_use]
    pub fn resolve(&self, name: &CapabilityId, req: &VersionReq) -> Option<String> {
        let providers = self.providers(name);
        resolve_capability(
            providers.iter().map(|(plugin, cap)| (plugin.as_str(), cap)),
            name,
            req,
        )
        .map(str::to_owned)
    }

    /// Removes `plugin`'s declarations from the registry, if any.
    pub fn remove(&self, plugin: &str) {
        self.provides.write().remove(plugin);
        self.requires.write().remove(plugin);
    }

    /// The `requires` entries of `plugin` that no registered provider
    /// satisfies, in declaration order. Soft entries are included: callers
    /// decide whether an unmet soft requirement matters (startup does not;
    /// only hard ones gate the plugin).
    #[must_use]
    pub fn unmet_requires(&self, plugin: &str) -> Vec<RequiredCapability> {
        let entries = self
            .requires
            .read()
            .get(plugin)
            .cloned()
            .unwrap_or_default();
        entries
            .iter()
            .filter(|req| self.resolve(&req.name, &req.req).is_none())
            .cloned()
            .collect()
    }

    /// Partitions `candidates` into committed and disabled plugins per the
    /// startup gate, removing disabled declarations from the registry.
    ///
    /// A plugin commits only when its hard `requires` are satisfied by the
    /// `provides` of plugins that also commit; the rest are disabled and
    /// their declarations removed, so a disabled plugin can never satisfy
    /// another plugin's `requires` or surface through
    /// [`resolve`](Self::resolve). Because dropping a provider can newly
    /// break the consumers that relied on it, the pass iterates until no
    /// further plugin is disabled (a fixpoint). Soft requirements never
    /// gate.
    ///
    /// Returns `(committed, disabled)`; only the committed declarations
    /// remain in the registry.
    #[must_use]
    pub fn gate(&self, candidates: &[String]) -> (Vec<String>, Vec<DisabledPlugin>) {
        let mut remaining = candidates.to_vec();
        let mut disabled = Vec::new();
        loop {
            let mut progressed = false;
            let mut survivors = Vec::with_capacity(remaining.len());
            for name in remaining {
                let unmet = self.unmet_requires(&name);
                if unmet.iter().any(|r| !r.soft) {
                    self.remove(&name);
                    disabled.push(DisabledPlugin { name, unmet });
                    progressed = true;
                } else {
                    survivors.push(name);
                }
            }
            remaining = survivors;
            if !progressed {
                return (remaining, disabled);
            }
        }
    }
}

/// A plugin the startup gate disabled for unmet hard capability requirements.
///
/// Carries the requirements that were unmet at the moment the plugin was
/// disabled so the caller can report them; soft entries are included
/// alongside the disabling hard ones but never cause a disablement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisabledPlugin {
    /// Plugin name.
    pub name: String,
    /// Unmet requirements at disablement time, in declaration order.
    pub unmet: Vec<RequiredCapability>,
}

/// Logs a warning for one rejected capability declaration entry.
///
/// One message per reason keeps a plugin with several bad declarations
/// attributable entry-by-entry instead of as a single blanket line.
fn warn_rejected_capability(plugin: &str, rejected: &RejectedCapability) {
    let entry = rejected.entry.as_deref().unwrap_or("<non-string>");
    match &rejected.reason {
        CapabilityRejection::InvalidEntry => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                "Ignoring capability declaration: entry is not a string"
            );
        }
        CapabilityRejection::InvalidName => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                entry = %entry,
                "Ignoring capability declaration: name is not a valid capability name"
            );
        }
        CapabilityRejection::MissingVersion => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                entry = %entry,
                "Ignoring capability declaration: missing '@' name/version separator"
            );
        }
        CapabilityRejection::MalformedVersion => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                entry = %entry,
                "Ignoring capability declaration: version/range is not valid semver, \
                 or uses a pre-release/build-metadata form"
            );
        }
        CapabilityRejection::SoftMarkerPosition => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                entry = %entry,
                "Ignoring capability declaration: '?' is only allowed as a single \
                 trailing marker on a requires entry"
            );
        }
        CapabilityRejection::Duplicate => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                entry = %entry,
                "Ignoring duplicate capability declaration; the first declaration wins"
            );
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
mod tests {
    use super::*;

    fn json_capabilities(provides: &[&str], requires: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "x-ene-capabilities": {
                "provides": provides,
                "requires": requires,
            }
        })
    }

    #[test]
    fn registers_only_valid_entries_and_skips_the_rest() {
        let registry = CapabilityRegistry::new();
        let schema = json_capabilities(
            &[
                "tts/synthesize@1",
                "bad name@1",
                "no-version",
                "dup@1",
                "dup@2",
            ],
            &["g2p/ja@^1", "bad@not-a-range", "soft@^1?"],
        );
        registry.register_from_schema("mock", Some(&schema));

        let providers = registry.providers(&CapabilityId::try_new("tts/synthesize").unwrap());
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].0, "mock");
        let dup = registry.providers(&CapabilityId::try_new("dup").unwrap());
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].1.version, semver::Version::new(1, 0, 0));

        let unmet = registry.unmet_requires("mock");
        // `g2p/ja` has no provider and `soft` is unmet-but-soft; both are
        // reported, only `g2p/ja` gates startup.
        assert_eq!(unmet.len(), 2);
        assert!(unmet.iter().any(|r| r.name.as_str() == "g2p/ja" && !r.soft));
        assert!(unmet.iter().any(|r| r.name.as_str() == "soft" && r.soft));
    }

    #[test]
    fn none_schema_registers_nothing() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("mock", None);
        assert!(registry.unmet_requires("mock").is_empty());
        assert!(
            registry
                .providers(&CapabilityId::try_new("anything").unwrap())
                .is_empty()
        );
    }

    #[test]
    fn none_schema_clears_previous_declarations() {
        let registry = CapabilityRegistry::new();
        let schema = json_capabilities(&["tts/synthesize@1"], &["g2p/ja@^1"]);
        registry.register_from_schema("mock", Some(&schema));
        let tts = CapabilityId::try_new("tts/synthesize").unwrap();
        assert_eq!(registry.providers(&tts).len(), 1);
        assert_eq!(
            registry.resolve(&tts, &"^1".parse().unwrap()),
            Some("mock".to_string())
        );
        assert_eq!(registry.unmet_requires("mock").len(), 1);

        registry.register_from_schema("mock", None);
        assert!(registry.providers(&tts).is_empty());
        assert_eq!(registry.resolve(&tts, &"^1".parse().unwrap()), None);
        assert!(registry.unmet_requires("mock").is_empty());
    }

    #[test]
    fn schema_without_capabilities_registers_empty_set() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("mock", Some(&serde_json::json!({ "type": "object" })));
        assert!(registry.unmet_requires("mock").is_empty());
        assert!(
            registry
                .resolve(
                    &CapabilityId::try_new("tts").unwrap(),
                    &"1".parse().unwrap()
                )
                .is_none()
        );
    }

    #[test]
    fn resolve_uses_providers_across_plugins() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema(
            "provider-a",
            Some(&json_capabilities(&["g2p/ja@2.1.0"], &[])),
        );
        registry.register_from_schema(
            "provider-b",
            Some(&json_capabilities(&["g2p/ja@2.2.0"], &[])),
        );

        let g2p = CapabilityId::try_new("g2p/ja").unwrap();
        // Both providers match `^2`; the lexicographically smallest plugin
        // name wins.
        assert_eq!(
            registry.resolve(&g2p, &"^2".parse().unwrap()),
            Some("provider-a".to_string())
        );
        // Only provider-b's 2.2.0 satisfies `^2.2`.
        assert_eq!(
            registry.resolve(&g2p, &"^2.2".parse().unwrap()),
            Some("provider-b".to_string())
        );
        assert_eq!(registry.resolve(&g2p, &"^3".parse().unwrap()), None);
    }

    #[test]
    fn unmet_requires_resolves_against_all_providers() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema(
            "llama",
            Some(&json_capabilities(&["gguf-runner@1.2.3"], &[])),
        );
        registry.register_from_schema(
            "consumer",
            Some(&json_capabilities(&[], &["gguf-runner@^1", "g2p/ja@^1?"])),
        );

        let unmet = registry.unmet_requires("consumer");
        // `gguf-runner@^1` is satisfied by llama; only the soft `g2p/ja` is
        // unmet (and it does not gate startup).
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].soft);
        assert_eq!(unmet[0].name.as_str(), "g2p/ja");
    }

    #[test]
    fn re_register_replaces_previous_entry() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("mock", Some(&json_capabilities(&["a@1"], &["x@^1"])));
        registry.register_from_schema("mock", Some(&json_capabilities(&["b@1"], &["y@^1"])));
        assert!(
            registry
                .providers(&CapabilityId::try_new("a").unwrap())
                .is_empty()
        );
        assert_eq!(
            registry
                .providers(&CapabilityId::try_new("b").unwrap())
                .len(),
            1
        );
        let unmet = registry.unmet_requires("mock");
        assert_eq!(unmet.len(), 1);
        assert_eq!(unmet[0].name.as_str(), "y");
    }

    #[test]
    fn remove_drops_declarations_and_resolution() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("mock", Some(&json_capabilities(&["a@1"], &["x@^1"])));
        registry.remove("mock");
        assert!(
            registry
                .providers(&CapabilityId::try_new("a").unwrap())
                .is_empty()
        );
        assert!(registry.unmet_requires("mock").is_empty());
    }

    #[test]
    fn gate_commits_plugins_whose_requires_are_satisfied() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("provider", Some(&json_capabilities(&["g2p/ja@2.1.0"], &[])));
        registry.register_from_schema(
            "consumer",
            Some(&json_capabilities(&[], &["g2p/ja@^2", "onnx-runner@^1?"])),
        );

        let (committed, disabled) =
            registry.gate(&["consumer".to_string(), "provider".to_string()]);
        // Both commit: the consumer's hard `g2p/ja` is satisfied and the
        // soft `onnx-runner` never gates; candidates keep their order.
        assert_eq!(
            committed,
            vec!["consumer".to_string(), "provider".to_string()]
        );
        assert!(disabled.is_empty());
        // Committed provides survive for the resolution API.
        let g2p = CapabilityId::try_new("g2p/ja").unwrap();
        assert_eq!(
            registry.resolve(&g2p, &"^2".parse().unwrap()),
            Some("provider".to_string())
        );
    }

    #[test]
    fn gate_disables_plugins_whose_provider_is_itself_disabled() {
        // H1 regression: P2 provides g2p/ja but fails its own hard
        // requirement; P1's `g2p/ja@^2` must not resolve to the disabled P2.
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("consumer", Some(&json_capabilities(&[], &["g2p/ja@^2"])));
        registry.register_from_schema(
            "provider",
            Some(&json_capabilities(&["g2p/ja@2.1.0"], &["onnx-runner@^1"])),
        );

        let (committed, disabled) =
            registry.gate(&["consumer".to_string(), "provider".to_string()]);

        // The provider cannot satisfy its own hard requirement and the
        // consumer cannot rely on a disabled provider, so both are disabled.
        assert!(committed.is_empty());
        assert_eq!(disabled.len(), 2);
        assert!(disabled.iter().any(|d| d.name == "consumer"));
        assert!(disabled.iter().any(|d| d.name == "provider"));
        let g2p = CapabilityId::try_new("g2p/ja").unwrap();
        assert_eq!(registry.resolve(&g2p, &"^2".parse().unwrap()), None);
        assert!(registry.providers(&g2p).is_empty());
    }

    #[test]
    fn gate_ignores_unmet_soft_requirements() {
        let registry = CapabilityRegistry::new();
        registry.register_from_schema(
            "consumer",
            Some(&json_capabilities(&[], &["tts/synthesize@^1?"])),
        );

        let (committed, disabled) = registry.gate(&["consumer".to_string()]);
        assert_eq!(committed, vec!["consumer".to_string()]);
        assert!(disabled.is_empty());
    }

    #[test]
    fn gate_iterates_to_a_fixpoint_across_the_dependency_chain() {
        // Each plugin's only provider fails its own hard requirement one
        // link down the chain; disabling propagates backwards until every
        // plugin is disabled, so no stale provides survives.
        let registry = CapabilityRegistry::new();
        registry.register_from_schema("a", Some(&json_capabilities(&[], &["x@^1"])));
        registry.register_from_schema("b", Some(&json_capabilities(&["x@1"], &["y@^1"])));
        registry.register_from_schema("c", Some(&json_capabilities(&["y@1"], &["z@^1"])));

        let (committed, disabled) =
            registry.gate(&["a".to_string(), "b".to_string(), "c".to_string()]);
        assert!(committed.is_empty());
        assert_eq!(disabled.len(), 3);
        let x = CapabilityId::try_new("x").unwrap();
        assert_eq!(registry.resolve(&x, &"^1".parse().unwrap()), None);
        assert!(registry.providers(&x).is_empty());
    }
}
