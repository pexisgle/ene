//! JSON Schema generation for [`Config`].
//!
//! Exposes the schema for tooling and validation. Nothing here writes files.

use schemars::Schema;

use crate::typed::Config;

/// Generates the JSON Schema describing [`Config`].
///
/// The schema carries no secret-typed properties and no runtime-judgment
/// properties such as autostart selection.
///
/// Note: the design doc names `schemars::schema::RootSchema` here, but the
/// pinned `schemars` 1.x exposes the root schema as [`Schema`]; this return
/// type is that same root schema under its current name.
pub fn config_schema() -> Schema {
    schemars::schema_for!(Config)
}

/// Returns [`config_schema`] as a [`serde_json::Value`] for embedding or
/// inspection.
///
/// Serialization failure is reported, never hidden: a schema that cannot be
/// represented is an error, not a null schema.
///
/// # Errors
///
/// Returns [`serde_json::Error`] when the schema cannot be serialized.
pub fn config_schema_json() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(config_schema())
}

#[cfg(test)]
mod tests {
    use super::{config_schema, config_schema_json};
    use crate::typed::Config;

    #[test]
    fn schema_is_generated_for_config() {
        let schema = config_schema();
        let json = config_schema_json();
        assert!(json.is_ok(), "schema serialization must succeed");
        let Some(json) = json.ok() else {
            return;
        };
        let title = match json.get("title") {
            Some(title) => title.as_str().unwrap_or_default().to_string(),
            None => String::new(),
        };
        assert!(
            title == "Config",
            "schema title must name Config, debug schema: {schema:?}"
        );
    }

    #[test]
    fn schema_declares_language_and_data_dir() {
        let json = config_schema_json();
        assert!(json.is_ok(), "schema serialization must succeed");
        let Some(value) = json.ok() else {
            return;
        };
        let properties = value.get("properties");
        assert!(
            properties.is_some(),
            "schema must declare properties: {value:?}"
        );
        let Some(properties) = properties else {
            return;
        };
        assert!(
            properties.get("language").is_some(),
            "schema must declare language: {value:?}"
        );
        assert!(
            properties.get("data_dir").is_some(),
            "schema must declare data_dir: {value:?}"
        );
    }

    #[test]
    fn schema_and_default_config_carry_no_secret_or_runtime_fields() {
        let schema = config_schema_json();
        assert!(schema.is_ok(), "schema serialization must succeed");
        let Some(schema) = schema.ok() else {
            return;
        };
        let default_value: Option<serde_json::Value> = serde_json::to_value(Config::default()).ok();
        assert!(
            default_value.is_some(),
            "a default Config must serialize to JSON"
        );
        let Some(default_value) = default_value else {
            return;
        };
        let properties = schema.get("properties");
        for forbidden in ["secret", "token", "password", "autostart"] {
            let in_schema = match properties {
                Some(properties) => properties.get(forbidden).is_some(),
                None => false,
            };
            assert!(!in_schema, "schema must not declare {forbidden}");
            assert!(
                default_value.get(forbidden).is_none(),
                "a default Config must not carry {forbidden}"
            );
        }
    }
}
