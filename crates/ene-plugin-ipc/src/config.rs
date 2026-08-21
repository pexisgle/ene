//! Optional plugin settings contract. Plugins that omit config keep working.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const SECRET_MARK: &str = "x-ene-secret";

/// JSON Schema (or a subset) the plugin advertises for its settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigSchema {
    #[serde(default)]
    pub has_config: bool,
    #[serde(default = "empty_object_schema")]
    pub schema: Value,
    /// Property names whose values are vault secrets, never returned to UI/logs.
    #[serde(default)]
    pub secret_keys: Vec<String>,
}

fn empty_object_schema() -> Value {
    json!({"type":"object","additionalProperties":false})
}

impl Default for PluginConfigSchema {
    fn default() -> Self {
        Self {
            has_config: false,
            schema: empty_object_schema(),
            secret_keys: Vec::new(),
        }
    }
}

/// One field-level validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigError {
    pub path: String,
    pub message: String,
}

/// Result of validating candidate settings without applying them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigValidateResult {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<PluginConfigError>,
    #[serde(default)]
    pub restart_required: bool,
}

impl PluginConfigValidateResult {
    #[must_use]
    pub fn ok() -> Self {
        Self {
            ok: true,
            errors: Vec::new(),
            restart_required: false,
        }
    }
}

/// One dynamic option for a config field (combo boxes, model lists, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigOption {
    pub id: String,
    pub label: String,
}

/// Dynamic options lookup. `fallback` is set when the plugin cannot enumerate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigOptionsResult {
    #[serde(default)]
    pub options: Vec<PluginConfigOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// UI should offer a free-text field when options are empty or `error` is set.
    #[serde(default)]
    pub fallback: bool,
}

impl PluginConfigOptionsResult {
    #[must_use]
    pub fn unsupported() -> Self {
        Self {
            options: Vec::new(),
            error: Some("dynamic options unavailable".to_owned()),
            fallback: true,
        }
    }
}

/// Apply outcome. On `ok: false` the host keeps the previous effective config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginConfigApplyResult {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<PluginConfigError>,
    #[serde(default)]
    pub restart_required: bool,
}

impl PluginConfigApplyResult {
    #[must_use]
    pub fn ok(restart_required: bool) -> Self {
        Self {
            ok: true,
            errors: Vec::new(),
            restart_required,
        }
    }
}

/// Collect property names marked `x-ene-secret` in a JSON Schema object.
#[must_use]
pub fn secret_keys_from_schema(schema: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    collect_secret_keys(schema, "", &mut keys);
    keys.sort();
    keys.dedup();
    keys
}

fn collect_secret_keys(schema: &Value, prefix: &str, keys: &mut Vec<String>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (name, child) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if is_secret_field(child) {
            keys.push(path.clone());
        }
        collect_secret_keys(child, &path, keys);
    }
}

fn is_secret_field(child: &Value) -> bool {
    child.get(SECRET_MARK) == Some(&Value::Bool(true))
        || child.get("writeOnly") == Some(&Value::Bool(true))
        || child.get("format").and_then(Value::as_str) == Some("password")
}

fn scrub_secret_examples(child: &mut Value) {
    if !is_secret_field(child) {
        return;
    }
    if let Some(obj) = child.as_object_mut() {
        obj.remove("default");
        obj.remove("examples");
        obj.remove("const");
    }
}

/// Drop `default` / `examples` / `const` on secret-marked schema properties.
#[must_use]
pub fn scrub_schema_secrets(schema: &Value) -> Value {
    let mut cloned = schema.clone();
    scrub_secret_tree(&mut cloned);
    cloned
}

fn scrub_secret_tree(schema: &mut Value) {
    scrub_secret_examples(schema);
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    let names: Vec<String> = properties.keys().cloned().collect();
    for name in names {
        if let Some(child) = properties.get_mut(&name) {
            scrub_secret_tree(child);
        }
    }
}

/// Drop secret fields from a settings object so it is safe for API, UI, and logs.
#[must_use]
pub fn redact_config_values(schema: &Value, values: &Value) -> Value {
    let secrets = secret_keys_from_schema(schema);
    redact_paths(values, &secrets)
}

fn redact_paths(values: &Value, secrets: &[String]) -> Value {
    let Value::Object(map) = values else {
        return values.clone();
    };
    let mut out = Map::new();
    for (key, value) in map {
        if secrets.iter().any(|secret| secret == key) {
            continue;
        }
        out.insert(key.clone(), redact_nested(key, value, secrets));
    }
    Value::Object(out)
}

fn redact_nested(prefix: &str, value: &Value, secrets: &[String]) -> Value {
    let Value::Object(map) = value else {
        return value.clone();
    };
    let mut out = Map::new();
    for (key, child) in map {
        let path = format!("{prefix}.{key}");
        if secrets.iter().any(|secret| secret == &path) {
            continue;
        }
        out.insert(key.clone(), redact_nested(&path, child, secrets));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::{PluginConfigSchema, redact_config_values, secret_keys_from_schema};
    use serde_json::json;

    #[test]
    fn default_schema_is_empty_object() {
        let schema = PluginConfigSchema::default();
        assert!(!schema.has_config);
        assert_eq!(schema.schema["type"], "object");
    }

    #[test]
    fn secret_fields_are_stripped_from_values() {
        let schema = json!({
            "type": "object",
            "properties": {
                "model": { "type": "string" },
                "api_key": { "type": "string", "x-ene-secret": true },
                "nested": {
                    "type": "object",
                    "properties": {
                        "token": { "type": "string", "format": "password" }
                    }
                }
            }
        });
        let values = json!({
            "model": "gpt",
            "api_key": "sk-live",
            "nested": { "token": "secret", "keep": true }
        });
        let keys = secret_keys_from_schema(&schema);
        assert!(keys.contains(&"api_key".to_owned()));
        assert!(keys.contains(&"nested.token".to_owned()));
        let redacted = redact_config_values(&schema, &values);
        assert_eq!(redacted["model"], "gpt");
        assert!(redacted.get("api_key").is_none());
        assert_eq!(redacted["nested"]["keep"], true);
        assert!(redacted["nested"].get("token").is_none());
        assert!(!format!("{redacted:?}").contains("sk-live"));
    }

    #[test]
    fn secret_schema_defaults_are_stripped() {
        let schema = json!({
            "type": "object",
            "properties": {
                "api_key": {
                    "type": "string",
                    "x-ene-secret": true,
                    "default": "sk-live"
                }
            }
        });
        let scrubbed = super::scrub_schema_secrets(&schema);
        assert!(scrubbed["properties"]["api_key"].get("default").is_none());
        assert!(!format!("{scrubbed:?}").contains("sk-live"));
    }
}
