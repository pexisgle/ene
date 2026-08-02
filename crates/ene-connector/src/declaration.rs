//! Parsing and scoping of `x-ene-credentials` credential declarations.
//!
//! A plugin declares the credentials it needs at the top level of the JSON
//! Schema returned by `config_schema()`:
//!
//! ```json
//! {
//!   "type": "object",
//!   "x-ene-credentials": [
//!     { "id": "anthropic", "kind": "api_key",
//!       "header": { "name": "x-api-key", "format": "{value}" } }
//!   ]
//! }
//! ```
//!
//! [`parse_credentials`] validates each entry independently — a bad entry is
//! reported and dropped, never the whole block — and [`resolve_scope`]
//! answers whether a plugin may access a given credential id. Both are pure:
//! the host drives the parsing and the credential service drives the scoping,
//! so neither side re-implements the rules.

use crate::identity::CredentialId;
use serde_json::Value;

/// The JSON Schema keyword under which a plugin declares its credentials.
pub const CREDENTIALS_KEY: &str = "x-ene-credentials";

/// The placeholder a `header.format` template must contain; the client
/// substitutes the stored value for it when injecting the header.
pub const VALUE_PLACEHOLDER: &str = "{value}";

/// A credential a plugin has declared via `x-ene-credentials`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialDeclaration {
    /// Stable id the plugin uses to request the credential
    /// (`anthropic`, `google.calendar`, …).
    pub id: CredentialId,
    /// How the credential is presented to the external service.
    pub kind: CredentialKind,
    /// Whether the plugin treats the credential as mandatory. Not enforced at
    /// startup; the credential service uses it for readiness signalling.
    pub required: bool,
    /// Whether other plugins declaring the same id address the same stored
    /// value (the default). A private declaration (`false`) resolves to
    /// `<plugin>:<id>`.
    pub shared: bool,
    /// Human-readable label for configuration UIs.
    pub label: Option<String>,
    /// Link to where the credential can be obtained.
    pub help_url: Option<String>,
}

/// The presentation kind of a declared credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialKind {
    /// A static secret the plugin receives as a header value.
    ApiKey {
        /// Header the client injects; `format` is a [`VALUE_PLACEHOLDER`]
        /// template.
        header: Option<HeaderSpec>,
        /// Environment variable the host may check when no value is stored.
        env_fallback: Option<String>,
    },
    /// An `OAuth2` flow driven by the host.
    OAuth2 {
        /// Public client identifier the authorization server requires even
        /// for PKCE flows (a desktop app ships no client secret).
        client_id: String,
        /// Permission scopes requested during consent.
        scopes: Vec<String>,
        /// Authorization endpoint for the consent redirect.
        auth_url: String,
        /// Token endpoint for token exchange and refresh.
        token_url: String,
    },
}

/// Header injection specification for an [`ApiKey`](CredentialKind::ApiKey)
/// credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderSpec {
    /// Header name, e.g. `x-api-key` or `Authorization`.
    pub name: String,
    /// Template containing [`VALUE_PLACEHOLDER`], e.g. `Bearer {value}`.
    pub format: String,
}

/// Why a single declaration entry was rejected during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRejection {
    /// The entry is not an object, or its `id` is missing or not a valid
    /// [`CredentialId`].
    InvalidId,
    /// `kind` is missing or not a supported kind.
    UnknownKind,
    /// An `oauth2` entry is missing the named required field.
    MissingOauth2Field(&'static str),
    /// An `api_key` entry's `header.format` lacks [`VALUE_PLACEHOLDER`].
    HeaderMissingPlaceholder,
    /// The `id` duplicates an earlier accepted entry in the same block.
    DuplicateId,
}

/// A declaration entry rejected during parsing, with enough context for the
/// host to warn about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedCredential {
    /// The raw `id` as written, when present.
    pub id: Option<String>,
    /// Why the entry was rejected.
    pub reason: CredentialRejection,
}

/// Why a declaration entry that was kept lost part of its configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialWarning {
    /// The entry's `header` object lacks the named required field (`name` or
    /// `format`); automatic header injection is disabled for the entry.
    HeaderMissingField(&'static str),
}

/// A declaration entry that was kept but lost part of its configuration,
/// with enough context for the host to warn about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DegradedCredential {
    /// The credential id of the affected entry.
    pub id: String,
    /// What was dropped from the entry.
    pub reason: CredentialWarning,
}

/// The outcome of parsing a plugin's `x-ene-credentials` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialParse {
    /// Accepted declarations, in declaration order (duplicates resolved
    /// first-wins).
    pub declarations: Vec<CredentialDeclaration>,
    /// Rejected entries, in declaration order.
    pub rejected: Vec<RejectedCredential>,
    /// Kept entries that lost part of their configuration (e.g. a malformed
    /// `header`), in declaration order; the host warns about each.
    pub degraded: Vec<DegradedCredential>,
}

/// Parses and validates the `x-ene-credentials` block of a plugin's config
/// schema.
///
/// Entries are validated independently: a bad entry lands in
/// [`CredentialParse::rejected`] and is dropped while valid entries are kept,
/// so one malformed declaration never disables the rest. An entry whose
/// `header` lost its `name` or `format` is kept but reported via
/// [`CredentialParse::degraded`] so the host can warn about the dropped
/// configuration. Duplicate ids keep the first occurrence. A schema without
/// `x-ene-credentials` (or with a non-array value) yields an empty parse, not
/// an error.
#[must_use]
pub fn parse_credentials(schema: &Value) -> CredentialParse {
    let Some(entries) = schema.get(CREDENTIALS_KEY).and_then(Value::as_array) else {
        return CredentialParse {
            declarations: Vec::new(),
            rejected: Vec::new(),
            degraded: Vec::new(),
        };
    };

    let mut declarations: Vec<CredentialDeclaration> = Vec::new();
    let mut rejected: Vec<RejectedCredential> = Vec::new();
    let mut degraded: Vec<DegradedCredential> = Vec::new();

    for entry in entries {
        let Some(entry) = entry.as_object() else {
            rejected.push(RejectedCredential {
                id: None,
                reason: CredentialRejection::InvalidId,
            });
            continue;
        };

        let Some(Value::String(raw_id)) = entry.get("id") else {
            rejected.push(RejectedCredential {
                id: None,
                reason: CredentialRejection::InvalidId,
            });
            continue;
        };

        let Ok(id) = CredentialId::try_new(raw_id.clone()) else {
            rejected.push(RejectedCredential {
                id: Some(raw_id.clone()),
                reason: CredentialRejection::InvalidId,
            });
            continue;
        };

        if declarations.iter().any(|d| d.id == id) {
            rejected.push(RejectedCredential {
                id: Some(raw_id.clone()),
                reason: CredentialRejection::DuplicateId,
            });
            continue;
        }

        let kind = match entry.get("kind").and_then(Value::as_str) {
            Some("api_key") => {
                let (header, degraded_reason) = parse_header(entry);
                if let Some(reason) = degraded_reason {
                    degraded.push(DegradedCredential {
                        id: raw_id.clone(),
                        reason,
                    });
                }
                if let Some(header) = &header
                    && !header.format.contains(VALUE_PLACEHOLDER)
                {
                    rejected.push(RejectedCredential {
                        id: Some(raw_id.clone()),
                        reason: CredentialRejection::HeaderMissingPlaceholder,
                    });
                    continue;
                }
                CredentialKind::ApiKey {
                    header,
                    env_fallback: entry
                        .get("env_fallback")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }
            }
            Some("oauth2") => {
                let Some(client_id) = entry.get("client_id").and_then(Value::as_str) else {
                    rejected.push(RejectedCredential {
                        id: Some(raw_id.clone()),
                        reason: CredentialRejection::MissingOauth2Field("client_id"),
                    });
                    continue;
                };
                let Some(auth_url) = entry.get("auth_url").and_then(Value::as_str) else {
                    rejected.push(RejectedCredential {
                        id: Some(raw_id.clone()),
                        reason: CredentialRejection::MissingOauth2Field("auth_url"),
                    });
                    continue;
                };
                let Some(token_url) = entry.get("token_url").and_then(Value::as_str) else {
                    rejected.push(RejectedCredential {
                        id: Some(raw_id.clone()),
                        reason: CredentialRejection::MissingOauth2Field("token_url"),
                    });
                    continue;
                };
                CredentialKind::OAuth2 {
                    client_id: client_id.to_owned(),
                    scopes: entry
                        .get("scopes")
                        .and_then(Value::as_array)
                        .map(|scopes| {
                            scopes
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                    auth_url: auth_url.to_owned(),
                    token_url: token_url.to_owned(),
                }
            }
            _ => {
                rejected.push(RejectedCredential {
                    id: Some(raw_id.clone()),
                    reason: CredentialRejection::UnknownKind,
                });
                continue;
            }
        };

        declarations.push(CredentialDeclaration {
            id,
            kind,
            required: entry
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            shared: entry.get("shared").and_then(Value::as_bool).unwrap_or(true),
            label: entry
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned),
            help_url: entry
                .get("help_url")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }

    CredentialParse {
        declarations,
        rejected,
        degraded,
    }
}

/// Parses an `api_key` entry's `header` object.
///
/// A well-formed header has a string `name` and `format`. A missing or
/// non-object `header` degrades to `None` silently — the declaration stays
/// valid with no automatic header injection. An object that lacks a usable
/// `name` or `format` also degrades to `None`, but reports a
/// [`CredentialWarning`] so the host can surface the dropped configuration
/// instead of silently accepting it.
fn parse_header(
    entry: &serde_json::Map<String, Value>,
) -> (Option<HeaderSpec>, Option<CredentialWarning>) {
    let Some(header) = entry.get("header").and_then(Value::as_object) else {
        return (None, None);
    };
    let Some(name) = header.get("name").and_then(Value::as_str) else {
        return (None, Some(CredentialWarning::HeaderMissingField("name")));
    };
    if name.is_empty() {
        return (None, Some(CredentialWarning::HeaderMissingField("name")));
    }
    let Some(format) = header.get("format").and_then(Value::as_str) else {
        return (None, Some(CredentialWarning::HeaderMissingField("format")));
    };
    (
        Some(HeaderSpec {
            name: name.to_owned(),
            format: format.to_owned(),
        }),
        None,
    )
}

/// The outcome of resolving a credential request against a plugin's declared
/// credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeDecision {
    /// The id is declared and the plugin may access the credential.
    Allowed {
        /// Storage key under which the value lives: the id itself for shared
        /// declarations, `<plugin>:<id>` for private ones. This is a
        /// vault-internal key, not a [`CredentialId`] — the `:` separator is
        /// outside the id charset, so the key cannot round-trip through
        /// validation. The vault must use it verbatim as its lookup key.
        storage_key: String,
    },
    /// The id is not declared; the request must be denied.
    Undeclared,
}

/// Resolves whether plugin `plugin_name` may access credential `id` given its
/// `declared` credentials.
///
/// Shared declarations resolve to the plain id, so plugins that declare the
/// same id address the same stored value. Private declarations
/// (`shared: false`) resolve to `<plugin>:<id>`, keeping each plugin's value
/// namespaced even when two plugins declare the same id. An undeclared id is
/// always denied — sharing is limited to plugins that declared the id.
///
/// The `:` separator is outside both the id charset (`[A-Za-z0-9._-]`) and the
/// plugin-name charset (`[A-Za-z0-9_-]`), so no shared id can be spelled like
/// a private key: plugin A's private `anthropic` (`A:anthropic`) is
/// structurally distinct from plugin C sharing the id `A.anthropic`
/// (`A.anthropic`). No separate uniqueness invariant is needed.
#[must_use]
pub fn resolve_scope(
    plugin_name: &str,
    declared: &[CredentialDeclaration],
    id: &CredentialId,
) -> ScopeDecision {
    let Some(decl) = declared.iter().find(|decl| &decl.id == id) else {
        return ScopeDecision::Undeclared;
    };
    if decl.shared {
        return ScopeDecision::Allowed {
            storage_key: id.to_string(),
        };
    }
    ScopeDecision::Allowed {
        storage_key: format!("{plugin_name}:{id}"),
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;

    fn parse_declarations(schema: &Value) -> CredentialParse {
        parse_credentials(schema)
    }

    #[test]
    fn parses_api_key_declaration() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{
                "id": "anthropic",
                "kind": "api_key",
                "required": true,
                "header": { "name": "x-api-key", "format": "{value}" },
                "env_fallback": "ANTHROPIC_API_KEY",
                "label": "Anthropic API Key",
                "help_url": "https://console.anthropic.com/settings/keys"
            }]
        }));
        assert!(parse.rejected.is_empty());
        let decl = &parse.declarations[0];
        assert_eq!(decl.id.as_str(), "anthropic");
        assert!(decl.required);
        assert!(decl.shared);
        assert_eq!(decl.label.as_deref(), Some("Anthropic API Key"));
        assert_eq!(
            decl.help_url.as_deref(),
            Some("https://console.anthropic.com/settings/keys")
        );
        match &decl.kind {
            CredentialKind::ApiKey {
                header,
                env_fallback,
            } => {
                let header = header.as_ref().unwrap();
                assert_eq!(header.name, "x-api-key");
                assert_eq!(header.format, "{value}");
                assert_eq!(env_fallback.as_deref(), Some("ANTHROPIC_API_KEY"));
            }
            CredentialKind::OAuth2 { .. } => panic!("expected api_key kind"),
        }
    }

    #[test]
    fn parses_oauth2_declaration() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{
                "id": "google.calendar",
                "kind": "oauth2",
                "client_id": "1234.apps.googleusercontent.com",
                "scopes": ["https://www.googleapis.com/auth/calendar.readonly"],
                "auth_url": "https://accounts.google.com/o/oauth2/v2/auth",
                "token_url": "https://oauth2.googleapis.com/token"
            }]
        }));
        assert!(parse.rejected.is_empty());
        let decl = &parse.declarations[0];
        assert_eq!(decl.id.as_str(), "google.calendar");
        assert!(!decl.required);
        assert!(decl.shared);
        match &decl.kind {
            CredentialKind::OAuth2 {
                client_id,
                scopes,
                auth_url,
                token_url,
            } => {
                assert_eq!(client_id, "1234.apps.googleusercontent.com");
                assert_eq!(scopes.len(), 1);
                assert_eq!(
                    scopes[0],
                    "https://www.googleapis.com/auth/calendar.readonly"
                );
                assert_eq!(auth_url, "https://accounts.google.com/o/oauth2/v2/auth");
                assert_eq!(token_url, "https://oauth2.googleapis.com/token");
            }
            CredentialKind::ApiKey { .. } => panic!("expected oauth2 kind"),
        }
    }

    #[test]
    fn defaults_are_required_false_and_shared_true() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key" }]
        }));
        assert!(parse.rejected.is_empty());
        let decl = &parse.declarations[0];
        assert!(!decl.required);
        assert!(decl.shared);
        assert!(decl.label.is_none());
        assert!(decl.help_url.is_none());
        match &decl.kind {
            CredentialKind::ApiKey {
                header,
                env_fallback,
            } => {
                assert!(header.is_none());
                assert!(env_fallback.is_none());
            }
            CredentialKind::OAuth2 { .. } => panic!("expected api_key kind"),
        }
    }

    #[test]
    fn hyphenated_id_is_accepted() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "google-calendar", "kind": "api_key" }]
        }));
        assert!(parse.rejected.is_empty());
        assert_eq!(parse.declarations[0].id.as_str(), "google-calendar");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{
                "id": "anthropic",
                "kind": "api_key",
                "bogus": { "nested": true },
                "another_unknown": 42
            }]
        }));
        assert!(parse.rejected.is_empty());
        assert_eq!(parse.declarations.len(), 1);
    }

    #[test]
    fn schema_without_credentials_key_yields_empty_parse() {
        let parse = parse_declarations(&serde_json::json!({
            "type": "object",
            "properties": { "voice": { "type": "string" } }
        }));
        assert!(parse.declarations.is_empty());
        assert!(parse.rejected.is_empty());
    }

    #[test]
    fn non_array_credentials_key_is_treated_as_absent() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": "not-an-array"
        }));
        assert!(parse.declarations.is_empty());
        assert!(parse.rejected.is_empty());
    }

    #[test]
    fn rejects_missing_or_invalid_id() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [
                { "kind": "api_key" },
                { "id": "bad id", "kind": "api_key" },
                42
            ]
        }));
        assert!(parse.declarations.is_empty());
        assert_eq!(parse.rejected.len(), 3);
        assert!(
            parse
                .rejected
                .iter()
                .all(|r| r.reason == CredentialRejection::InvalidId)
        );
    }

    #[test]
    fn rejects_unknown_kind() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [
                { "id": "x", "kind": "magic" },
                { "id": "y" }
            ]
        }));
        assert!(parse.declarations.is_empty());
        assert_eq!(parse.rejected.len(), 2);
        assert!(
            parse
                .rejected
                .iter()
                .all(|r| r.reason == CredentialRejection::UnknownKind)
        );
        assert_eq!(parse.rejected[0].id.as_deref(), Some("x"));
        assert_eq!(parse.rejected[1].id.as_deref(), Some("y"));
    }

    #[test]
    fn rejects_oauth2_missing_required_fields() {
        let mut missing_client_id = serde_json::json!({
            "id": "google.calendar", "kind": "oauth2",
            "auth_url": "https://a", "token_url": "https://t"
        });
        missing_client_id["client_id"] = serde_json::json!("1234.apps.googleusercontent.com");
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [missing_client_id]
        }));
        assert!(parse.declarations.is_empty());
        assert_eq!(
            parse.rejected[0].reason,
            CredentialRejection::MissingOauth2Field("client_id")
        );

        let base = serde_json::json!({
            "id": "google.calendar", "kind": "oauth2",
            "client_id": "1234.apps.googleusercontent.com"
        });
        let mut missing_auth = base.clone();
        missing_auth["token_url"] = serde_json::json!("https://t");
        let mut missing_token = base;
        missing_token["auth_url"] = serde_json::json!("https://a");

        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [missing_auth, missing_token]
        }));
        assert!(parse.declarations.is_empty());
        assert_eq!(
            parse.rejected,
            vec![
                RejectedCredential {
                    id: Some("google.calendar".to_string()),
                    reason: CredentialRejection::MissingOauth2Field("auth_url"),
                },
                RejectedCredential {
                    id: Some("google.calendar".to_string()),
                    reason: CredentialRejection::MissingOauth2Field("token_url"),
                },
            ]
        );
    }

    #[test]
    fn rejects_header_format_without_placeholder() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{
                "id": "anthropic",
                "kind": "api_key",
                "header": { "name": "x-api-key", "format": "plain-value" }
            }]
        }));
        assert!(parse.declarations.is_empty());
        assert_eq!(
            parse.rejected[0].reason,
            CredentialRejection::HeaderMissingPlaceholder
        );
    }

    #[test]
    fn malformed_header_degrades_to_no_injection() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [
                { "id": "a", "kind": "api_key", "header": { "name": "", "format": "{value}" } },
                { "id": "b", "kind": "api_key", "header": { "format": "{value}" } },
                { "id": "c", "kind": "api_key", "header": 42 },
                { "id": "d", "kind": "api_key" },
                { "id": "e", "kind": "api_key", "header": { "name": "x-api-key" } }
            ]
        }));
        assert!(parse.rejected.is_empty());
        assert_eq!(parse.declarations.len(), 5);
        for decl in &parse.declarations {
            let CredentialKind::ApiKey { header, .. } = &decl.kind else {
                panic!("expected api_key kind");
            };
            assert!(header.is_none());
        }
        // A header *object* that lost its name or format is surfaced so the
        // host can warn; a missing or non-object header stays silent.
        assert_eq!(
            parse.degraded,
            vec![
                DegradedCredential {
                    id: "a".to_string(),
                    reason: CredentialWarning::HeaderMissingField("name"),
                },
                DegradedCredential {
                    id: "b".to_string(),
                    reason: CredentialWarning::HeaderMissingField("name"),
                },
                DegradedCredential {
                    id: "e".to_string(),
                    reason: CredentialWarning::HeaderMissingField("format"),
                },
            ]
        );
    }

    #[test]
    fn duplicate_id_keeps_first_occurrence() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [
                { "id": "anthropic", "kind": "api_key", "shared": false },
                { "id": "anthropic", "kind": "oauth2", "auth_url": "https://a", "token_url": "https://t" }
            ]
        }));
        assert_eq!(parse.declarations.len(), 1);
        assert!(!parse.declarations[0].shared);
        assert_eq!(parse.rejected.len(), 1);
        assert_eq!(
            parse.rejected[0],
            RejectedCredential {
                id: Some("anthropic".to_string()),
                reason: CredentialRejection::DuplicateId,
            }
        );
    }

    #[test]
    fn resolve_scope_shared_resolves_to_plain_id() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key" }]
        }));
        let id = CredentialId::try_new("anthropic").unwrap();
        assert_eq!(
            resolve_scope("plugin-a", &parse.declarations, &id),
            ScopeDecision::Allowed {
                storage_key: "anthropic".to_string()
            }
        );
    }

    #[test]
    fn resolve_scope_private_resolves_to_colon_separated_key() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key", "shared": false }]
        }));
        let id = CredentialId::try_new("anthropic").unwrap();
        assert_eq!(
            resolve_scope("plugin-a", &parse.declarations, &id),
            ScopeDecision::Allowed {
                storage_key: "plugin-a:anthropic".to_string()
            }
        );
        // Two private declarations of the same id stay distinct per plugin.
        assert_eq!(
            resolve_scope("plugin-b", &parse.declarations, &id),
            ScopeDecision::Allowed {
                storage_key: "plugin-b:anthropic".to_string()
            }
        );
    }

    #[test]
    fn resolve_scope_private_key_cannot_collide_with_shared_id() {
        // Plugin A's private "anthropic" resolves to `A:anthropic`; plugin C
        // sharing `A.anthropic` as an id resolves to the plain id
        // `A.anthropic`. The `:` separator is outside the id charset, so no
        // shared id can be spelled like a private key and a private value is
        // never reachable through a shared declaration.
        let private = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key", "shared": false }]
        }));
        let shared = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "A.anthropic", "kind": "api_key" }]
        }));
        let anthropic = CredentialId::try_new("anthropic").unwrap();
        let dotted = CredentialId::try_new("A.anthropic").unwrap();
        let private_key = match resolve_scope("A", &private.declarations, &anthropic) {
            ScopeDecision::Allowed { storage_key } => storage_key,
            ScopeDecision::Undeclared => panic!("private declaration must resolve"),
        };
        let shared_key = match resolve_scope("C", &shared.declarations, &dotted) {
            ScopeDecision::Allowed { storage_key } => storage_key,
            ScopeDecision::Undeclared => panic!("shared declaration must resolve"),
        };
        assert_eq!(private_key, "A:anthropic");
        assert_eq!(shared_key, "A.anthropic");
        assert_ne!(private_key, shared_key);
    }

    #[test]
    fn resolve_scope_undeclared_id_is_denied() {
        let parse = parse_declarations(&serde_json::json!({
            "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key" }]
        }));
        let other = CredentialId::try_new("openai").unwrap();
        assert_eq!(
            resolve_scope("plugin-a", &parse.declarations, &other),
            ScopeDecision::Undeclared
        );
        // An empty declaration set denies everything.
        assert_eq!(
            resolve_scope("plugin-a", &[], &other),
            ScopeDecision::Undeclared
        );
    }
}
