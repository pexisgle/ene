//! Host-side registry of per-plugin credential declarations.
//!
//! [`CredentialRegistry`] maps plugin names to the credential declarations
//! parsed from each plugin's `x-ene-credentials` schema block at startup. It
//! wraps [`resolve_scope`](ene_connector::declaration::resolve_scope) so the
//! credential service can answer "may this plugin access this id" without
//! re-parsing schemas.

use std::collections::HashMap;

use ene_connector::declaration::{
    CredentialDeclaration, CredentialRejection, CredentialWarning, DegradedCredential,
    RejectedCredential, ScopeDecision, parse_credentials,
};
use ene_connector::identity::CredentialId;
use parking_lot::RwLock;
use serde_json::Value;

/// Registered credential declarations keyed by plugin name.
#[derive(Debug, Default)]
pub struct CredentialRegistry {
    declarations: RwLock<HashMap<String, Vec<CredentialDeclaration>>>,
}

impl CredentialRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `schema`'s `x-ene-credentials` block and records the result for
    /// `plugin`, replacing any previous entry.
    ///
    /// Rejected entries are warned about individually and dropped — one bad
    /// declaration never affects the rest, and the plugin itself is never
    /// involved. Entries that kept only part of their configuration (e.g. a
    /// `header` missing `name`) are kept but warned about. A `None` schema
    /// clears any previous entry for `plugin`.
    pub fn register_from_schema(&self, plugin: &str, schema: Option<&Value>) {
        let Some(schema) = schema else {
            self.register(plugin, Vec::new());
            return;
        };
        let parse = parse_credentials(schema);
        for rejected in &parse.rejected {
            warn_rejected_credential(plugin, rejected);
        }
        for degraded in &parse.degraded {
            warn_degraded_credential(plugin, degraded);
        }
        self.register(plugin, parse.declarations);
    }

    /// Records `declarations` for `plugin`, replacing any previous entry.
    ///
    /// Idempotent: re-registering the same plugin (e.g. after a schema
    /// re-parse) simply overwrites.
    pub fn register(&self, plugin: &str, declarations: Vec<CredentialDeclaration>) {
        self.declarations
            .write()
            .insert(plugin.to_string(), declarations);
    }

    /// Returns the declarations registered for `plugin`, or an empty list
    /// when the plugin registered none.
    #[must_use]
    pub fn declarations(&self, plugin: &str) -> Vec<CredentialDeclaration> {
        self.declarations
            .read()
            .get(plugin)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the names of every plugin that has a registered declaration
    /// entry.
    #[must_use]
    pub fn plugins(&self) -> Vec<String> {
        self.declarations.read().keys().cloned().collect()
    }

    /// Returns the declaration registered for `plugin` under `id`, if any.
    #[must_use]
    pub fn declaration(&self, plugin: &str, id: &str) -> Option<CredentialDeclaration> {
        self.declarations(plugin)
            .into_iter()
            .find(|decl| decl.id.as_str() == id)
    }

    /// Returns every declaration across all plugins, deduplicated by id
    /// (first registration wins).
    #[must_use]
    pub fn all_declarations(&self) -> Vec<CredentialDeclaration> {
        let mut plugins: Vec<String> = self.declarations.read().keys().cloned().collect();
        plugins.sort();
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        for plugin in plugins {
            for decl in self.declarations(&plugin) {
                if seen.insert(decl.id.to_string()) {
                    all.push(decl);
                }
            }
        }
        all
    }

    /// Returns the first plugin sharing `id` through a `shared` declaration,
    /// alongside its declaration.
    ///
    /// Used by flows started without a requesting plugin (the desktop
    /// settings page), where any sharer qualifies.
    #[must_use]
    pub fn find_shared_declaration(
        &self,
        id: &CredentialId,
    ) -> Option<(CredentialDeclaration, String)> {
        let mut plugins: Vec<String> = self.declarations.read().keys().cloned().collect();
        plugins.sort();
        for plugin in plugins {
            if let Some(decl) = self.declaration(&plugin, id.as_str())
                && decl.shared
            {
                return Some((decl, plugin));
            }
        }
        None
    }

    /// Resolves the declaration serving a vault storage key.
    ///
    /// Shared keys are plain ids — any plugin declaring the id qualifies.
    /// Private keys are `<plugin>:<id>` (the `:` is outside the id charset,
    /// so the split is unambiguous) and resolve to that plugin's own
    /// declaration. Used by the token refresher, which only sees storage
    /// keys.
    #[must_use]
    pub fn declaration_for_storage_key(&self, storage_key: &str) -> Option<CredentialDeclaration> {
        if let Some((plugin, id)) = storage_key.split_once(':') {
            return self.declaration(plugin, id);
        }
        let id = CredentialId::try_new(storage_key).ok()?;
        self.find_shared_declaration(&id).map(|(decl, _)| decl)
    }

    /// Resolves whether `plugin` may access credential `id`, per the
    /// declarations registered for it.
    #[must_use]
    pub fn resolve_scope(&self, plugin: &str, id: &CredentialId) -> ScopeDecision {
        let declared = self.declarations(plugin);
        ene_connector::declaration::resolve_scope(plugin, &declared, id)
    }
}

/// Logs a warning for one rejected declaration entry.
///
/// One message per reason keeps a plugin with several bad declarations
/// attributable entry-by-entry instead of as a single blanket line.
fn warn_rejected_credential(plugin: &str, rejected: &RejectedCredential) {
    let credential_id = rejected.id.as_deref().unwrap_or("<missing>");
    match &rejected.reason {
        CredentialRejection::InvalidId => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                credential_id = %credential_id,
                "Ignoring credential declaration: id is missing or not a valid credential id"
            );
        }
        CredentialRejection::UnknownKind => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                credential_id = %credential_id,
                "Ignoring credential declaration: kind is missing or not a supported credential kind"
            );
        }
        CredentialRejection::MissingOauth2Field(field) => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                credential_id = %credential_id,
                field = %field,
                "Ignoring credential declaration: oauth2 requires the field"
            );
        }
        CredentialRejection::HeaderMissingPlaceholder => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                credential_id = %credential_id,
                "Ignoring credential declaration: header.format must contain the {{value}} placeholder"
            );
        }
        CredentialRejection::DuplicateId => {
            tracing::warn!(
                component = "PluginHostManager",
                plugin = %plugin,
                credential_id = %credential_id,
                "Ignoring duplicate credential declaration; the first declaration wins"
            );
        }
    }
}

/// Logs a warning for a declaration entry that was kept but lost part of its
/// configuration (e.g. a `header` object missing `name` or `format`).
fn warn_degraded_credential(plugin: &str, degraded: &DegradedCredential) {
    let credential_id = degraded.id.as_str();
    // Single-variant pattern: `HeaderMissingField` is currently the only
    // warning, so a plain `let` bind is irrefutable.
    let CredentialWarning::HeaderMissingField(field) = &degraded.reason;
    tracing::warn!(
        component = "PluginHostManager",
        plugin = %plugin,
        credential_id = %credential_id,
        field = %field,
        "Keeping credential declaration without header injection: header field missing or empty"
    );
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests use expect/unwrap for concise failure messages"
)]
mod tests {
    use super::*;

    fn json_credentials(entries: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "x-ene-credentials": entries })
    }

    #[test]
    fn registers_only_valid_entries_and_skips_the_rest() {
        let registry = CredentialRegistry::new();
        let schema = json_credentials(&serde_json::json!([
            { "id": "anthropic", "kind": "api_key", "header": { "name": "x-api-key", "format": "{value}" } },
            { "id": "bad id", "kind": "api_key" },
            { "id": "google.calendar", "kind": "oauth2", "client_id": "client-id", "auth_url": "https://a", "token_url": "https://t" },
            { "id": "broken.header", "kind": "api_key", "header": { "name": "x-api-key", "format": "no-placeholder" } },
            { "id": "dup", "kind": "api_key" },
            { "id": "dup", "kind": "api_key" }
        ]));
        registry.register_from_schema("mock", Some(&schema));

        let declarations = registry.declarations("mock");
        assert_eq!(declarations.len(), 3);
        assert_eq!(declarations[0].id.as_str(), "anthropic");
        assert_eq!(declarations[1].id.as_str(), "google.calendar");
        assert_eq!(declarations[2].id.as_str(), "dup");
    }

    #[test]
    fn none_schema_registers_nothing() {
        let registry = CredentialRegistry::new();
        registry.register_from_schema("mock", None);
        assert!(registry.declarations("mock").is_empty());
    }

    #[test]
    fn schema_without_credentials_registers_empty_set() {
        let registry = CredentialRegistry::new();
        registry.register_from_schema("mock", Some(&serde_json::json!({ "type": "object" })));
        assert!(registry.declarations("mock").is_empty());
        let id = CredentialId::try_new("anthropic").unwrap();
        assert_eq!(
            registry.resolve_scope("mock", &id),
            ScopeDecision::Undeclared
        );
    }

    #[test]
    fn resolve_scope_uses_registered_declarations() {
        let registry = CredentialRegistry::new();
        let schema = json_credentials(&serde_json::json!([
            { "id": "shared.key", "kind": "api_key" },
            { "id": "private.key", "kind": "api_key", "shared": false }
        ]));
        registry.register_from_schema("plugin-a", Some(&schema));

        let shared = CredentialId::try_new("shared.key").unwrap();
        let private = CredentialId::try_new("private.key").unwrap();
        assert_eq!(
            registry.resolve_scope("plugin-a", &shared),
            ScopeDecision::Allowed {
                storage_key: "shared.key".to_string()
            }
        );
        assert_eq!(
            registry.resolve_scope("plugin-a", &private),
            ScopeDecision::Allowed {
                storage_key: "plugin-a:private.key".to_string()
            }
        );
        // An undeclared plugin is denied even though another declared the id.
        assert_eq!(
            registry.resolve_scope("plugin-b", &shared),
            ScopeDecision::Undeclared
        );
    }

    #[test]
    fn re_register_replaces_previous_entry() {
        let registry = CredentialRegistry::new();
        let first = json_credentials(&serde_json::json!([{ "id": "a", "kind": "api_key" }]));
        let second = json_credentials(&serde_json::json!([{ "id": "b", "kind": "api_key" }]));
        registry.register_from_schema("mock", Some(&first));
        registry.register_from_schema("mock", Some(&second));
        let declarations = registry.declarations("mock");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].id.as_str(), "b");
    }

    #[test]
    fn none_schema_clears_previous_declarations() {
        let registry = CredentialRegistry::new();
        let schema = json_credentials(&serde_json::json!([{ "id": "a", "kind": "api_key" }]));
        registry.register_from_schema("mock", Some(&schema));
        let id = CredentialId::try_new("a").unwrap();
        assert_eq!(
            registry.resolve_scope("mock", &id),
            ScopeDecision::Allowed {
                storage_key: "a".to_string()
            }
        );

        registry.register_from_schema("mock", None);
        assert!(registry.declarations("mock").is_empty());
        assert_eq!(
            registry.resolve_scope("mock", &id),
            ScopeDecision::Undeclared
        );
    }

    #[test]
    fn declaration_for_storage_key_resolves_shared_and_private() {
        let registry = CredentialRegistry::new();
        let schema = json_credentials(&serde_json::json!([
            { "id": "google.calendar", "kind": "oauth2", "client_id": "c", "auth_url": "https://a", "token_url": "https://t" },
            { "id": "private.key", "kind": "oauth2", "client_id": "c", "auth_url": "https://a", "token_url": "https://t", "shared": false }
        ]));
        registry.register_from_schema("plugin-a", Some(&schema));

        let shared = registry
            .declaration_for_storage_key("google.calendar")
            .expect("shared storage key resolves");
        assert_eq!(shared.id.as_str(), "google.calendar");

        let private = registry
            .declaration_for_storage_key("plugin-a:private.key")
            .expect("private storage key resolves");
        assert_eq!(private.id.as_str(), "private.key");

        assert!(registry.declaration_for_storage_key("nope").is_none());
        assert!(
            registry
                .declaration_for_storage_key("other:private.key")
                .is_none()
        );
    }

    #[test]
    fn find_shared_declaration_returns_first_sharer_only() {
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "plugin-b",
            Some(&json_credentials(
                &serde_json::json!([{ "id": "shared.key", "kind": "api_key" }]),
            )),
        );
        registry.register_from_schema(
            "plugin-a",
            Some(&json_credentials(
                &serde_json::json!([{ "id": "shared.key", "kind": "api_key" }]),
            )),
        );
        registry.register_from_schema(
            "private-only",
            Some(&json_credentials(
                &serde_json::json!([{ "id": "shared.key", "kind": "api_key", "shared": false }]),
            )),
        );
        let id = CredentialId::try_new("shared.key").unwrap();
        let (decl, plugin) = registry
            .find_shared_declaration(&id)
            .expect("shared declaration found");
        assert_eq!(decl.id, id);
        assert_eq!(plugin, "plugin-a");
    }
}
