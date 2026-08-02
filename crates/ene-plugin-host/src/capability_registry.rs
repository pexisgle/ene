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
    /// involved. A `None` schema registers nothing.
    pub fn register_from_schema(&self, plugin: &str, schema: Option<&Value>) {
        let Some(schema) = schema else {
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

    /// The `requires` entries of `plugin` that no registered provider
    /// satisfies, in declaration order. Soft entries are included: callers
    /// decide whether an unmet soft requirement matters (startup does not;
    /// only hard ones gate the plugin).
    #[must_use]
    pub fn unmet_requires(&self, plugin: &str) -> Vec<RequiredCapability> {
        let requires = self.requires.read();
        let Some(entries) = requires.get(plugin) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter(|req| self.resolve(&req.name, &req.req).is_none())
            .cloned()
            .collect()
    }
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
            Some(&json_capabilities(&["g2p/ja@1.5.0"], &[])),
        );

        let g2p = CapabilityId::try_new("g2p/ja").unwrap();
        // Both providers match `^1`; the lexicographically smallest plugin
        // name wins.
        assert_eq!(
            registry.resolve(&g2p, &"^1".parse().unwrap()),
            Some("provider-a".to_string())
        );
        // Only provider-a's 2.1.0 satisfies `^2`.
        assert_eq!(
            registry.resolve(&g2p, &"^2".parse().unwrap()),
            Some("provider-a".to_string())
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
}
