//! Settings draft — the single edit surface for the settings window.
//!
//! Pages never mutate the persisted [`crate::settings::CharacterSettings`] config directly
//! (that was the previous architecture). Instead every config write lands on
//! a [`SettingsDraft`], which tracks dirty paths, runs schema validation,
//! owns a monotonic revision for stale-apply detection, and distinguishes
//! secret states so real secret values never round-trip back into the UI.
//! Applying a draft persists local `desktop.*` settings and patches dirty
//! core sections through `ene-api`.
#![expect(
    dead_code,
    reason = "draft helpers stay for secret/revision flows that core settings have not started using"
)]

use ene_config::EneConfig;
use std::collections::{BTreeMap, BTreeSet};

/// Placeholder standing in for a stored secret inside the draft's
/// persisted/editing copies. Real secret values live only in the on-disk
/// store and are merged back at apply time. NUL bytes make a collision with
/// a genuine value impossible.
pub const SECRET_PLACEHOLDER: &str = "\u{0}ene-secret-placeholder\u{0}";

/// Well-known secret-bearing key names, matched case-insensitively as
/// substrings (fail-safe: masking a non-secret is harmless, leaking one is
/// not).
const SECRET_KEY_NAMES: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "token",
    "access_token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "auth",
    "credential",
    "credential_file",
];

fn is_secret_key_name(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEY_NAMES
        .iter()
        .any(|candidate| lower.contains(candidate))
}

/// State of a secret-bearing field inside the draft.
///
/// The real secret value lives only in the persisted config (or in the
/// `api_key.inline`-style fields that stay in the editing copy but are never
/// echoed into UI text buffers). The UI distinguishes these states so an
/// "unchanged" secret is not overwritten by an empty text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecretState {
    /// Not a secret, or no secret-specific handling engaged.
    #[default]
    None,
    /// The secret is present and unchanged; the UI must not show the value.
    Unchanged,
    /// The user chose to delete the stored secret.
    Deleted,
    /// The user supplied a replacement value.
    Replaced,
    /// The value comes from an environment variable source (`source: "env"`).
    EnvSource,
}

/// Field-level impact classification shown next to fields and on apply
/// results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldImpact {
    /// Takes effect as soon as the field changes (appearance, character).
    #[default]
    Immediate,
    /// Hot-reloads a runtime subsystem without restarting anything.
    RuntimeReload,
    /// Requires a plugin-host restart.
    PluginRestart,
    /// Requires an app restart.
    AppRestart,
}

impl FieldImpact {
    /// Parses an `x-ene-ui.impact` value (`"immediate"`, `"runtime_reload"`,
    /// `"plugin_restart"`, `"app_restart"`).
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "immediate" => Some(Self::Immediate),
            "runtime_reload" => Some(Self::RuntimeReload),
            "plugin_restart" => Some(Self::PluginRestart),
            "app_restart" => Some(Self::AppRestart),
            _ => None,
        }
    }

    /// Stable English code for i18n lookup keys.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::RuntimeReload => "runtime_reload",
            Self::PluginRestart => "plugin_restart",
            Self::AppRestart => "app_restart",
        }
    }
}

/// One validation issue tied to a dotted config path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftIssue {
    /// Dotted path the issue belongs to (section key, or `section.field`).
    pub path: String,
    /// Human-readable message (already localized where possible).
    pub message: String,
}

#[derive(Debug)]
pub struct SettingsDraft {
    /// Config the draft was last synced from (persisted values).
    persisted: EneConfig,
    /// Working copy being edited.
    editing: EneConfig,
    /// Section keys (or dotted paths) edited since the last sync/apply.
    dirty_paths: BTreeSet<String>,
    /// Validation issues keyed by dotted path.
    validation: BTreeMap<String, Vec<String>>,
    /// Monotonic revision; bumped on every edit and echoed on every apply.
    revision: u64,
    /// Revision of the last successful apply.
    applied_revision: u64,
    /// The actor-side settings revision this draft is based on; sent as
    /// `base_revision` so a stale draft is rejected instead of silently
    /// overwriting newer settings.
    actor_revision: u64,
    /// Secret states by dotted path.
    secrets: BTreeMap<String, SecretState>,
}

impl SettingsDraft {
    /// Creates a draft whose persisted and editing copies both start at
    /// `persisted`.
    #[must_use]
    pub fn new(persisted: EneConfig) -> Self {
        let redacted = redact_config_for_draft(&persisted);
        Self {
            persisted: redacted.clone(),
            editing: redacted,
            dirty_paths: BTreeSet::new(),
            validation: BTreeMap::new(),
            revision: 0,
            applied_revision: 0,
            actor_revision: 0,
            secrets: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn editing(&self) -> &EneConfig {
        &self.editing
    }

    /// The config the draft was last synced from (the rollback baseline).
    #[must_use]
    pub const fn persisted(&self) -> &EneConfig {
        &self.persisted
    }

    #[must_use]
    pub fn section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + ene_config::HasConfigKey,
    {
        self.editing.get_section::<T>().unwrap_or_default()
    }

    /// Writes a typed section into the working copy and records it dirty.
    pub fn set_section<T>(&mut self, section: &T)
    where
        T: serde::Serialize + ene_config::HasConfigKey,
    {
        if self.editing.set_section(section).is_ok() {
            self.touch(T::KEY.to_string());
        }
    }

    /// Writes a dotted path into the working copy and records it dirty.
    ///
    /// Top-level declared `EneConfig` fields (`character`, `user_name`,
    /// `runtime_rules`, `user_persona`) live outside the flattened `extra`
    /// map and are routed to the struct field; the actor's apply diff
    /// compares those fields directly, so writing them into `extra` would
    /// silently no-op the live apply.
    pub fn set_path(&mut self, dotted_path: &str, value: serde_json::Value) {
        match dotted_path {
            "character" => {
                self.editing.character = value.as_str().unwrap_or_default().to_string();
                self.touch(dotted_path.to_string());
                return;
            }
            "user_name" => {
                self.editing.user_name = value.as_str().unwrap_or_default().to_string();
                self.touch(dotted_path.to_string());
                return;
            }
            "runtime_rules" => {
                self.editing.runtime_rules = value.as_str().unwrap_or_default().to_string();
                self.touch(dotted_path.to_string());
                return;
            }
            "user_persona" => {
                let Ok(persona) = serde_json::from_value(value) else {
                    return;
                };
                self.editing.user_persona = Some(persona);
                self.touch(dotted_path.to_string());
                return;
            }
            _ => {}
        }
        let Ok(json) = serde_json::to_string(&value) else {
            return;
        };
        if self.editing.set_path(dotted_path, &json).is_ok() {
            self.touch(dotted_path.to_string());
        }
    }

    /// Writes a whole section as an opaque JSON value (schema-form path).
    pub fn set_section_value(&mut self, key: &str, value: serde_json::Value) {
        if self.editing.set_section_value(key, value).is_ok() {
            self.touch(key.to_string());
        }
    }

    /// Seeds a core section fetched from the daemon into both copies without
    /// marking the draft dirty. Used so the AI / features pages can edit
    /// live core JSON without treating the fetch as a user change.
    pub fn seed_core_section(&mut self, key: &str, value: serde_json::Value) {
        drop(self.editing.set_section_value(key, value.clone()));
        drop(self.persisted.set_section_value(key, value));
    }

    fn touch(&mut self, path: String) {
        self.dirty_paths.insert(path);
        self.revision = self.revision.saturating_add(1);
    }

    #[must_use]
    pub fn dirty_paths(&self) -> &BTreeSet<String> {
        &self.dirty_paths
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.dirty_paths.is_empty()
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn applied_revision(&self) -> u64 {
        self.applied_revision
    }

    #[must_use]
    pub const fn actor_revision(&self) -> u64 {
        self.actor_revision
    }

    pub fn set_actor_revision(&mut self, actor_revision: u64) {
        self.actor_revision = actor_revision;
    }

    /// Marks the current revision as applied and clears pending state.
    ///
    /// The caller must refresh [`SettingsDraft::resync`] with the persisted
    /// config afterwards so `persisted` tracks the new on-disk state.
    pub fn mark_applied(&mut self) {
        self.applied_revision = self.revision;
        self.dirty_paths.clear();
        self.validation.clear();
        self.secrets
            .retain(|_, state| *state == SecretState::Unchanged);
    }

    pub fn resync(&mut self, persisted: EneConfig) {
        let redacted = redact_config_for_draft(&persisted);
        self.persisted = redacted.clone();
        self.editing = redacted;
        self.dirty_paths.clear();
        self.validation.clear();
        self.secrets
            .retain(|_, state| *state == SecretState::Unchanged);
    }

    /// Refreshes only the persisted baseline, keeping pending edits intact.
    ///
    /// Used when a stale-draft conflict is detected: the user's edits stay
    /// in the working copy so they can review and re-apply against the
    /// newer baseline.
    pub fn resync_baseline(&mut self, persisted: EneConfig) {
        self.persisted = redact_config_for_draft(&persisted);
    }

    pub fn set_secret(&mut self, path: &str, state: SecretState) {
        if state == SecretState::None {
            self.secrets.remove(path);
        } else {
            self.secrets.insert(path.to_string(), state);
        }
    }

    /// Defaults to [`SecretState::None`].
    #[must_use]
    pub fn secret(&self, path: &str) -> SecretState {
        self.secrets.get(path).copied().unwrap_or_default()
    }

    /// Runs subset JSON-Schema validation over every dirty section and
    /// refreshes the validation map. Non-dirty sections keep their previous
    /// issues (if any) so a fixed field does not silently re-flag.
    pub fn validate(&mut self) {
        let dirty: Vec<String> = self.dirty_paths.iter().cloned().collect();
        for path in dirty {
            let section_key = path.split('.').next().unwrap_or(&path).to_string();
            let mut issues = Vec::new();
            validate_section(&self.editing, &section_key, &mut issues);
            if issues.is_empty() {
                self.validation.remove(&section_key);
            } else {
                self.validation.insert(section_key, issues);
            }
        }
    }

    #[must_use]
    pub fn issues_for(&self, section_key: &str) -> &[String] {
        self.validation
            .get(section_key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn has_issues(&self) -> bool {
        !self.validation.is_empty()
    }

    /// Returned in deterministic order.
    #[must_use]
    pub fn all_issues(&self) -> Vec<DraftIssue> {
        self.validation
            .iter()
            .flat_map(|(path, messages)| {
                messages
                    .iter()
                    .map(|message| DraftIssue {
                        path: path.clone(),
                        message: message.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Default impact for a section key, used when the schema carries no
    /// `x-ene-ui.impact` metadata.
    #[must_use]
    pub fn default_impact_for(section_key: &str) -> FieldImpact {
        match section_key {
            "plugins" => FieldImpact::RuntimeReload,
            "ai" | "mind" | "store" | "rag" => FieldImpact::RuntimeReload,
            _ => FieldImpact::Immediate,
        }
    }

    /// Reads `x-ene-ui.impact` from a section schema, falling back to
    /// [`Self::default_impact_for`].
    #[must_use]
    pub fn impact_for(section_key: &str) -> FieldImpact {
        let Some((_, entry)) =
            ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings)
                .into_iter()
                .find(|(key, _)| key == section_key)
        else {
            return Self::default_impact_for(section_key);
        };
        let Ok(schema) = serde_json::to_value(&entry.schema) else {
            return Self::default_impact_for(section_key);
        };
        schema
            .pointer("/x-ene-ui/impact")
            .and_then(serde_json::Value::as_str)
            .and_then(FieldImpact::parse)
            .unwrap_or_else(|| Self::default_impact_for(section_key))
    }
}

/// Runs the subset JSON-Schema validator over one section of `config`.
///
/// Shared by [`SettingsDraft::validate`] and the async apply preparation so
/// both paths validate the merged (secret-restored) config identically.
pub(crate) fn validate_section(config: &EneConfig, section_key: &str, issues: &mut Vec<String>) {
    let Some((_, entry)) =
        ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings)
            .into_iter()
            .find(|(key, _)| key == section_key)
    else {
        return;
    };
    let Ok(schema) = serde_json::to_value(&entry.schema) else {
        return;
    };
    let Some(value) = config.section_value(section_key) else {
        return;
    };
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .and_then(serde_json::Value::as_object);
    validate_value(&schema, &value, section_key, defs, issues);
}

/// Returns a copy of `config` with every stored secret replaced by
/// [`SECRET_PLACEHOLDER`], so real values never live in UI state.
///
/// Redaction rules (fail-safe, schema-independent):
/// - string leaves whose key matches a well-known secret name are masked;
/// - `api_key` descriptor objects keep `source` / `env` editable and mask
///   only the stored `inline` value;
/// - `credentials` maps mask every value;
/// - arrays recurse into **all** elements.
fn redact_config_for_draft(config: &EneConfig) -> EneConfig {
    let mut redacted = config.clone();
    for (key, value) in &mut redacted.extra {
        redact_value(key, value);
    }
    redacted
}

fn redact_value(key: &str, value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(string) => {
            if is_secret_key_name(key) && !string.is_empty() {
                *string = SECRET_PLACEHOLDER.to_string();
            }
        }
        serde_json::Value::Object(object) => {
            if key == "api_key" {
                if let Some(inline) = object.get_mut("inline")
                    && inline.is_string()
                    && !inline.as_str().unwrap_or_default().is_empty()
                {
                    *inline = serde_json::Value::String(SECRET_PLACEHOLDER.to_string());
                }
                return;
            }
            if key == "credentials" {
                for value in object.values_mut() {
                    if value.is_string() && !value.as_str().unwrap_or_default().is_empty() {
                        *value = serde_json::Value::String(SECRET_PLACEHOLDER.to_string());
                    }
                }
                return;
            }
            if is_secret_key_name(key) {
                if !object.is_empty() {
                    *value = serde_json::Value::String(SECRET_PLACEHOLDER.to_string());
                }
                return;
            }
            for (child_key, child) in object.iter_mut() {
                redact_value(child_key, child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                match item {
                    serde_json::Value::Object(object) => {
                        for (child_key, child) in object.iter_mut() {
                            redact_value(child_key, child);
                        }
                    }
                    serde_json::Value::Array(_) => redact_value(key, item),
                    serde_json::Value::String(string) => {
                        if is_secret_key_name(key) && !string.is_empty() {
                            *string = SECRET_PLACEHOLDER.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) fn config_with_real_secrets() -> EneConfig {
    let mut config = EneConfig::default();
    drop(config.set_section_value(
        "ai",
        serde_json::json!({
            "providers": {
                "openai": {
                    "api_key": {
                        "source": "inline",
                        "inline": "sk-stored-inline"
                    }
                }
            }
        }),
    ));
    drop(config.set_section_value(
        "plugins",
        serde_json::json!({
            "list": {
                "demo": {
                    "credentials": { "cred-key": "cred-stored-value" },
                    "config": { "token": "config-stored-token", "voice": "af_heart" }
                }
            },
            "mcp_servers": [{
                "name": "m",
                "enabled": true,
                "transport": { "url": "http://example.test", "auth_header": "Bearer stored-auth" }
            }]
        }),
    ));
    config
}

/// Validates `value` against a subset of JSON Schema (draft-07 style):
/// `type` (single or array), `enum`, `required`, `properties`, `items`,
/// numeric bounds, string lengths, `pattern`, `format: url`, and `$ref`
/// resolution against the schema's `$defs` / `definitions`. Unknown keys in
/// objects are preserved and never rejected.
///
/// The subset deliberately covers what the generated schemas use; a schema
/// construct outside it is treated as pass-through rather than a hard error.
pub fn validate_value(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
    defs: Option<&serde_json::Map<String, serde_json::Value>>,
    issues: &mut Vec<String>,
) {
    if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
        let resolved = resolve_ref(reference, defs);
        if let Some(resolved) = resolved {
            validate_value(&resolved, value, path, defs, issues);
        }
        return;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(serde_json::Value::as_array) {
        let mut matched = false;
        for variant in one_of {
            let mut variant_issues = Vec::new();
            validate_value(variant, value, path, defs, &mut variant_issues);
            if variant_issues.is_empty() {
                matched = true;
                break;
            }
        }
        if !matched {
            issues.push(format!("{path}: does not match any oneOf variant"));
        }
        return;
    }

    let types = match schema.get("type") {
        Some(serde_json::Value::String(t)) => vec![t.as_str()],
        Some(serde_json::Value::Array(types)) => types
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    if !types.is_empty() && !types.iter().any(|t| value_matches_type(value, t)) {
        issues.push(format!("{path}: expected {}", types.join(" or ")));
        return;
    }

    if let Some(enum_values) = schema.get("enum").and_then(serde_json::Value::as_array)
        && !enum_values.iter().any(|candidate| candidate == value)
    {
        issues.push(format!("{path}: value is not one of the allowed choices"));
    }

    match value {
        serde_json::Value::Object(object) => {
            if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
                for field in required {
                    if let Some(name) = field.as_str()
                        && !object.contains_key(name)
                    {
                        issues.push(format!("{path}.{name}: required field is missing"));
                    }
                }
            }
            if let Some(properties) = schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                for (name, property_schema) in properties {
                    if let Some(child) = object.get(name) {
                        validate_value(
                            property_schema,
                            child,
                            &format!("{path}.{name}"),
                            defs,
                            issues,
                        );
                    }
                }
            }
        }
        serde_json::Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate_value(item_schema, item, &format!("{path}[{index}]"), defs, issues);
                }
            }
        }
        serde_json::Value::String(string) => {
            let length = string.chars().count();
            if let Some(min) = schema.get("minLength").and_then(serde_json::Value::as_u64)
                && u64::try_from(length).is_ok_and(|len| len < min)
            {
                issues.push(format!("{path}: shorter than {min} characters"));
            }
            if let Some(max) = schema.get("maxLength").and_then(serde_json::Value::as_u64)
                && u64::try_from(length).is_ok_and(|len| len > max)
            {
                issues.push(format!("{path}: longer than {max} characters"));
            }
            if let Some(pattern) = schema.get("pattern").and_then(serde_json::Value::as_str)
                && regex::Regex::new(pattern).is_ok_and(|re| !re.is_match(string))
            {
                issues.push(format!("{path}: does not match the required pattern"));
            }
            if schema
                .get("format")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|format| format == "url" && !looks_like_url(string))
            {
                issues.push(format!("{path}: not a valid URL"));
            }
        }
        serde_json::Value::Number(number) => {
            if let Some(min) = schema.get("minimum").and_then(serde_json::Value::as_f64)
                && number.as_f64().is_some_and(|n| n < min)
            {
                issues.push(format!("{path}: below the minimum of {min}"));
            }
            if let Some(max) = schema.get("maximum").and_then(serde_json::Value::as_f64)
                && number.as_f64().is_some_and(|n| n > max)
            {
                issues.push(format!("{path}: above the maximum of {max}"));
            }
        }
        serde_json::Value::Null => {}
        serde_json::Value::Bool(_) => {}
    }
}

fn value_matches_type(value: &serde_json::Value, type_name: &str) -> bool {
    match type_name {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn resolve_ref(
    reference: &str,
    defs: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Value> {
    let name = reference.strip_prefix("#/$defs/").or_else(|| {
        reference.strip_prefix("#/definitions/").or_else(|| {
            reference
                .strip_prefix("#/")
                .and_then(|rest| rest.split('/').next_back())
        })
    })?;
    defs?.get(name).cloned()
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://") || value.starts_with("file://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn draft() -> SettingsDraft {
        SettingsDraft::new(ene_config::EneConfig::default())
    }

    #[test]
    fn set_section_value_tracks_dirty_paths_and_revision() {
        let mut d = draft();
        d.set_section_value("mind", json!({"session": {"session_timeout_minutes": 30}}));
        assert!(d.is_dirty());
        assert!(d.dirty_paths().contains("mind"));
        assert_eq!(d.revision(), 1);
        assert_eq!(
            d.editing()
                .section_value("mind")
                .and_then(|value| value.pointer("/session/session_timeout_minutes").cloned()),
            Some(json!(30))
        );
    }

    #[test]
    fn set_path_routes_top_level_fields_to_declared_fields() {
        let mut d = draft();
        d.set_path("user_name", json!("Alice"));
        assert!(d.dirty_paths().contains("user_name"));
        assert_eq!(d.editing().user_name, "Alice");
        assert!(
            !d.editing().extra.contains_key("user_name"),
            "top-level fields must not leak into `extra`"
        );
        d.set_path("runtime_rules", json!("be concise"));
        assert_eq!(d.editing().runtime_rules, "be concise");
    }

    #[test]
    fn mark_applied_and_resync_clear_pending_state() {
        let mut d = draft();
        d.set_path("user_name", json!("Alice"));
        assert_eq!(d.revision(), 1);
        d.mark_applied();
        assert!(!d.is_dirty());
        assert_eq!(d.applied_revision(), 1);
        d.resync(ene_config::EneConfig::default());
        assert!(!d.is_dirty());
        assert_eq!(
            d.revision(),
            1,
            "resync keeps the revision counter monotonic"
        );
    }

    #[test]
    fn secret_states_round_trip() {
        let mut d = draft();
        assert_eq!(d.secret("ai.providers.openai.api_key"), SecretState::None);
        d.set_secret("ai.providers.openai.api_key", SecretState::Unchanged);
        assert_eq!(
            d.secret("ai.providers.openai.api_key"),
            SecretState::Unchanged
        );
        d.set_secret("ai.providers.openai.api_key", SecretState::None);
        assert_eq!(d.secret("ai.providers.openai.api_key"), SecretState::None);
    }

    #[test]
    fn validate_catches_type_and_enum_violations() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 2},
                "mode": {"type": "string", "enum": ["auto", "manual"]},
                "port": {"type": "integer", "minimum": 1}
            },
            "required": ["name"]
        });
        let mut issues = Vec::new();
        validate_value(
            &schema,
            &json!({"name": "x", "mode": "other", "port": 0}),
            "demo",
            None,
            &mut issues,
        );
        assert_eq!(
            issues.len(),
            3,
            "minLength, enum, and minimum all fail: {issues:?}"
        );
    }

    #[test]
    fn validate_preserves_unknown_keys() {
        let schema = json!({"type": "object", "properties": {"known": {"type": "string"}}});
        let mut issues = Vec::new();
        validate_value(
            &schema,
            &json!({"known": "ok", "unknown_extra": 42}),
            "demo",
            None,
            &mut issues,
        );
        assert!(
            issues.is_empty(),
            "unknown keys must not be rejected: {issues:?}"
        );
    }

    #[test]
    fn validate_resolves_local_refs() {
        let schema = json!({
            "$defs": {"Inner": {"type": "object", "properties": {"v": {"type": "integer"}}}},
            "type": "object",
            "properties": {"inner": {"$ref": "#/$defs/Inner"}}
        });
        let mut issues = Vec::new();
        validate_value(
            &schema,
            &json!({"inner": {"v": "not an integer"}}),
            "demo",
            schema.get("$defs").and_then(serde_json::Value::as_object),
            &mut issues,
        );
        assert_eq!(issues.len(), 1, "nested ref violation reported: {issues:?}");
    }

    #[test]
    fn gpu_layers_schema_accepts_auto_and_integers() {
        let schema = json!({
            "oneOf": [
                {"type": "string", "enum": ["auto"]},
                {"type": "integer", "minimum": 0, "maximum": 4_294_967_295_u64}
            ]
        });
        for value in [json!("auto"), json!(0), json!(33)] {
            let mut issues = Vec::new();
            validate_value(&schema, &value, "gpu_layers", None, &mut issues);
            assert!(issues.is_empty(), "expected {value} to pass: {issues:?}");
        }
        for value in [json!("bogus"), json!(-1), json!(true)] {
            let mut issues = Vec::new();
            validate_value(&schema, &value, "gpu_layers", None, &mut issues);
            assert!(!issues.is_empty(), "expected {value} to fail");
        }
    }

    #[test]
    fn impact_metadata_is_parsed() {
        assert_eq!(
            FieldImpact::parse("runtime_reload"),
            Some(FieldImpact::RuntimeReload)
        );
        assert_eq!(FieldImpact::parse("bogus"), None);
        assert_eq!(
            SettingsDraft::default_impact_for("ai"),
            FieldImpact::RuntimeReload
        );
        assert_eq!(
            SettingsDraft::default_impact_for("desktop"),
            FieldImpact::Immediate
        );
        assert_eq!(
            SettingsDraft::default_impact_for("plugins"),
            FieldImpact::RuntimeReload
        );
    }

    #[test]
    fn draft_never_holds_stored_secret_values() {
        let draft = SettingsDraft::new(config_with_real_secrets());
        let text = serde_json::to_string(draft.editing()).expect("editing serializes");
        for secret in [
            "sk-stored-inline",
            "cred-stored-value",
            "config-stored-token",
            "stored-auth",
        ] {
            assert!(
                !text.contains(secret),
                "stored secret `{secret}` must never reach the draft"
            );
        }
        assert!(
            text.contains("ene-secret-placeholder"),
            "the placeholder (JSON-escaped) must appear in the serialized draft"
        );
        assert_eq!(
            draft
                .editing()
                .get_path("ai.providers.openai.api_key.source")
                .and_then(|value| value.as_str().map(str::to_owned)),
            Some("inline".to_owned())
        );
        assert_eq!(
            draft
                .editing()
                .get_path("ai.providers.openai.api_key.inline")
                .and_then(|value| value.as_str().map(str::to_owned))
                .as_deref(),
            Some(SECRET_PLACEHOLDER)
        );
    }

    #[test]
    fn seed_core_section_does_not_dirty() {
        let mut d = draft();
        d.seed_core_section("ai", json!({"tasks": {"chat": {"provider": "echo"}}}));
        assert!(!d.is_dirty());
        assert_eq!(
            d.editing()
                .section_value("ai")
                .and_then(|value| value.pointer("/tasks/chat/provider").cloned()),
            Some(json!("echo"))
        );
    }
}
