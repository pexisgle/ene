//! Redaction of plugin configuration values at the host boundary (#313).
//!
//! A plugin's `config_schema()` may mark fields with `x-ene-secret: true`; the
//! host must never log those values, and the UI must mask them. This module
//! provides the schema-aware redaction used by host logging paths, plus a
//! key-name fallback so secrets stay masked even when a plugin ships no schema
//! (defense in depth).

/// Well-known secret-bearing key names, matched (case-insensitively) even when
/// a plugin schema does not mark the field `x-ene-secret: true`.
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

/// Replacement value written over every redacted secret.
const REDACTED: &str = "***";

/// Returns `true` when the schema property for `key` declares
/// `x-ene-secret: true`.
fn is_secret_marked(schema: Option<&serde_json::Value>, key: &str) -> bool {
    schema
        .and_then(|s| s.get("properties"))
        .and_then(|props| props.get(key))
        .and_then(|prop| prop.get("x-ene-secret"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Returns `true` when `key` matches a well-known secret name.
///
/// Substring matching (case-insensitive) is deliberate: host logging must
/// fail *safe* — masking a non-secret key is harmless, while a single leaked
/// API key is not. This catches `api_key`, `apiKey`, `ANTHROPIC_API_KEY`,
/// `access_token`, `nested.password`, and so on.
fn is_secret_key_name(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEY_NAMES
        .iter()
        .any(|candidate| lower.contains(candidate))
}

/// Redacts secret values in a plugin configuration blob.
///
/// A key is redacted when the optional `schema` marks its property
/// `x-ene-secret: true`, or when the key matches a well-known secret name.
/// Redacted values are replaced wholesale with `"***"` (including nested
/// objects such as the `{"source": "inline", "inline": "…"}` api-key shape, so
/// the embedded value can never leak). Everything else is preserved and
/// recursed into.
///
/// This is a pure, allocation-friendly function used by host logging paths and
/// (later) by the settings UI to mask secret fields.
#[must_use]
pub fn redact_config(
    config: &serde_json::Value,
    schema: Option<&serde_json::Value>,
) -> serde_json::Value {
    match config {
        serde_json::Value::Object(obj) => {
            let mut out = serde_json::Map::with_capacity(obj.len());
            for (key, value) in obj {
                if is_secret_marked(schema, key) || is_secret_key_name(key) {
                    out.insert(key.clone(), serde_json::Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_config(value, schema));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(|v| redact_config(v, schema)).collect())
        }
        other => other.clone(),
    }
}

/// Redacts a config blob without a schema (host logging fallback).
#[must_use]
pub fn redact_config_unschematized(config: &serde_json::Value) -> serde_json::Value {
    redact_config(config, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_schema_marked_secrets() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "api_key": { "type": "string", "x-ene-secret": true },
                "base_url": { "type": "string" }
            }
        });
        let config = serde_json::json!({
            "api_key": "sk-ant-secret-123",
            "base_url": "https://api.example.com"
        });
        let redacted = redact_config(&config, Some(&schema));
        assert_eq!(redacted["api_key"], serde_json::json!("***"));
        assert_eq!(
            redacted["base_url"],
            serde_json::json!("https://api.example.com")
        );
    }

    #[test]
    fn redacts_well_known_secret_names_without_schema() {
        let config = serde_json::json!({
            "api_key": {"source": "inline", "inline": "sk-inline-value"},
            "ANTHROPIC_API_KEY": "sk-env-var-value",
            "voice": "af_heart",
            "nested": {"password": "hunter2", "keep": 42}
        });
        let redacted = redact_config_unschematized(&config);
        assert_eq!(redacted["api_key"], serde_json::json!("***"));
        assert_eq!(redacted["ANTHROPIC_API_KEY"], serde_json::json!("***"));
        assert_eq!(redacted["voice"], serde_json::json!("af_heart"));
        assert_eq!(redacted["nested"]["password"], serde_json::json!("***"));
        assert_eq!(redacted["nested"]["keep"], serde_json::json!(42));
    }

    #[test]
    fn redacts_secret_keys_inside_nested_objects() {
        // The api_key inline shape must be masked wholesale so the embedded
        // value can never appear in a log line.
        let config = serde_json::json!({
            "providers": {
                "default": {
                    "api_key": {"source": "inline", "inline": "sk-deep-secret"}
                }
            }
        });
        let redacted = redact_config_unschematized(&config);
        assert_eq!(
            redacted["providers"]["default"]["api_key"],
            serde_json::json!("***")
        );
    }

    #[test]
    fn redacts_arrays_recursively() {
        let config = serde_json::json!({"keys": [{"api_key": "a"}, {"api_key": "b"}]});
        let redacted = redact_config_unschematized(&config);
        assert_eq!(redacted["keys"][0]["api_key"], serde_json::json!("***"));
        assert_eq!(redacted["keys"][1]["api_key"], serde_json::json!("***"));
    }

    #[test]
    fn non_secret_config_is_preserved_verbatim() {
        let config = serde_json::json!({"mmproj_url": "https://cdn.example/mmproj.gguf", "n": 3});
        assert_eq!(redact_config_unschematized(&config), config);
    }
}
