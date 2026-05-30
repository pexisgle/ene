use crate::error::ConfigError;
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::Path;

/// Global singleton holding the active [`EneConfig`].
///
/// Set once at startup by [`load_full_config`] and updated by
/// [`update_global_config`]. Accessed by [`get_global_config`]
/// and [`get_global_section`].
pub static GLOBAL_CONFIG: std::sync::OnceLock<std::sync::RwLock<EneConfig>> =
    std::sync::OnceLock::new();

/// Updates the global EneConfig
pub fn update_global_config(config: EneConfig) {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = config;
        }
    } else {
        let _ = GLOBAL_CONFIG.set(std::sync::RwLock::new(config));
    }
}

/// Gets a clone of the entire global config
pub fn get_global_config() -> EneConfig {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        if let Ok(guard) = lock.read() {
            return guard.clone();
        }
    }
    EneConfig::default()
}

/// Trait for config structs that possess a unique config key.
pub trait HasConfigKey {
    /// The string key of this configuration section under the root config's `extra` map.
    const KEY: &'static str;
}

/// Loads a subsection from the global config using the type's associated key.
pub fn get_global_section<T>() -> T
where
    T: serde::de::DeserializeOwned + Default + HasConfigKey,
{
    if let Some(lock) = GLOBAL_CONFIG.get() {
        if let Ok(guard) = lock.read() {
            return guard.get_section::<T>().unwrap_or_default();
        }
    }
    T::default()
}

/// Loads a subsection by key from the global config.
pub fn get_global_section_by_key<T: serde::de::DeserializeOwned + Default>(key: &str) -> T {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        if let Ok(guard) = lock.read() {
            return guard.get_section_by_key::<T>(key).unwrap_or_default();
        }
    }
    T::default()
}

/// A registered config schema entry.
pub struct SchemaEntry {
    /// The JSON Schema definition for this config section.
    pub schema: schemars::Schema,
    /// Optional parent key under which this schema should be nested at merge time.
    pub parent: Option<String>,
}

static SCHEMA_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, SchemaEntry>>> =
    std::sync::OnceLock::new();

/// Registry holding schemas collected from tools at runtime
static RUNTIME_SCHEMA_REGISTRY: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, SchemaEntry>>,
> = std::sync::OnceLock::new();

/// Global static helper for each crate to register its own Config schema
pub fn register_schema<T: JsonSchema>(key: &str) {
    register_schema_inner::<T>(key, None);
}

/// Registers schemas collected from tools at runtime
pub fn register_runtime_schema(key: &str, schema: serde_json::Value, parent: Option<String>) {
    let root_schema: schemars::Schema = serde_json::from_value(schema).unwrap_or_else(|e| {
        tracing::error!("Failed to parse runtime schema for '{}': {}", key, e);
        schemars::Schema::default()
    });
    let registry = RUNTIME_SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(
            key.to_string(),
            SchemaEntry {
                schema: root_schema,
                parent,
            },
        );
    }
}

/// Registers nested within a parent schema's definition/property
pub fn register_schema_with_parent<T: JsonSchema>(key: &str, parent: &str) {
    register_schema_inner::<T>(key, Some(parent.to_string()));
}

fn register_schema_inner<T: JsonSchema>(key: &str, parent: Option<String>) {
    let schema_gen = schemars::SchemaGenerator::default();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(key.to_string(), SchemaEntry { schema, parent });
    }
}

fn merge_child_into_parent(
    root_schema: &mut schemars::Schema,
    parent_key: &str,
    child_schema: &schemars::Schema,
) {
    let child_props = child_schema
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned();
    let child_props = match child_props {
        Some(p) if !p.is_empty() => p,
        _ => return,
    };

    // Try $defs first (e.g. "ToolEntry")
    if let Some(defs) = root_schema.get_mut("$defs").and_then(|v| v.as_object_mut()) {
        if let Some(def) = defs.get_mut(parent_key) {
            if let Some(def_props) = def.get_mut("properties").and_then(|v| v.as_object_mut()) {
                for (k, v) in &child_props {
                    def_props.insert(k.clone(), v.clone());
                }
                return;
            }
        }
    }

    // Fall back to root-level property
    if let Some(root_obj) = root_schema.as_object_mut() {
        if let Some(parent_val) = root_obj
            .get_mut("properties")
            .and_then(|v| v.get_mut(parent_key))
        {
            if let Some(parent_props) = parent_val
                .get_mut("properties")
                .and_then(|v| v.as_object_mut())
            {
                for (k, v) in child_props {
                    parent_props.insert(k, v);
                }
            }
        }
    }
}

crate::define_config!(
    "ene_config",
    /// Top-level application configuration for ene.
    pub struct EneConfig {
        /// Schema version number.
        pub version: u32 = 1,
        /// Character card name or path.
        pub character: String = String::new(),
        /// Display name shown to the user.
        pub user_name: String = "User".to_string(),
        /// Behavioural rules injected into every system prompt.
        pub runtime_rules: String = "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.".to_string(),

        #[serde(flatten)]
        #[schemars(skip)]
        /// Catch-all for provider, tool, and other sub-configurations.
        pub extra: HashMap<String, serde_json::Value> = HashMap::new(),
    }
);

impl EneConfig {
    /// Deserialise a sub-section from the `extra` map using the type's associated key.
    ///
    /// Returns `Ok(T::default())` when the key is absent.
    pub fn get_section<T>(&self) -> Result<T, ConfigError>
    where
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        self.get_section_by_key(T::KEY)
    }

    /// Deserialise a sub-section from the `extra` map by key.
    ///
    /// Returns `Ok(T::default())` when the key is absent.
    pub fn get_section_by_key<T>(&self, key: &str) -> Result<T, ConfigError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        if let Some(val) = self.extra.get(key) {
            serde_json::from_value(val.clone()).map_err(|e| {
                ConfigError::GenericConfigError(format!(
                    "Failed to deserialize section '{}': {}",
                    key, e
                ))
            })
        } else {
            Ok(T::default())
        }
    }

    /// Serialise and insert a sub-section into the `extra` map using the type's associated key.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), ConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        self.set_section_by_key(T::KEY, section)
    }

    /// Serialise and insert a sub-section into the `extra` map by key.
    pub fn set_section_by_key<T>(&mut self, key: &str, section: &T) -> Result<(), ConfigError>
    where
        T: serde::Serialize,
    {
        let val = serde_json::to_value(section).map_err(|e| {
            ConfigError::GenericConfigError(format!("Failed to serialize section '{}': {}", key, e))
        })?;
        self.extra.insert(key.to_string(), val);
        Ok(())
    }

    /// Extracts a nested field value from the provider section in the `extra` map.
    ///
    /// This avoids the manual `extra["provider"]["field"]` chain duplicated across
    /// embedding and memory config resolvers.
    pub fn get_provider_field(&self, key: &str) -> Option<String> {
        self.extra
            .get("provider")
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
    }
}

fn apply_registry_to_schema(
    root_schema: &mut schemars::Schema,
    registry: &HashMap<String, SchemaEntry>,
) {
    // First pass: insert entries without parents and collect all definitions
    for (key, entry) in registry.iter() {
        if entry.parent.is_none() {
            if let Some(root_obj) = root_schema.as_object_mut() {
                let props = root_obj
                    .entry("properties".to_string())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(props_obj) = props.as_object_mut() {
                    props_obj.insert(key.clone(), entry.schema.clone().into());
                }
            }
        }
        // Copy $defs from child schema into root schema
        if let Some(child_defs) = entry.schema.get("$defs").and_then(|v| v.as_object()) {
            let root_defs = root_schema
                .ensure_object()
                .entry("$defs".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(root_defs_obj) = root_defs.as_object_mut() {
                for (def_name, def_schema) in child_defs {
                    root_defs_obj.insert(def_name.clone(), def_schema.clone());
                }
            }
        }
    }
    // Second pass: merge entries that have parents
    for (_, entry) in registry.iter() {
        if let Some(parent_key) = &entry.parent {
            merge_child_into_parent(root_schema, parent_key, &entry.schema);
        }
    }
}

/// Generates the JSON representation of the JSON Schema
pub fn generate_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let mut root_schema = schema_gen.into_root_schema_for::<EneConfig>();

    if let Some(registry) = SCHEMA_REGISTRY.get() {
        if let Ok(reg) = registry.lock() {
            apply_registry_to_schema(&mut root_schema, &reg);
        }
    }

    if let Some(registry) = RUNTIME_SCHEMA_REGISTRY.get() {
        if let Ok(reg) = registry.lock() {
            apply_registry_to_schema(&mut root_schema, &reg);
        }
    }

    serde_json::to_string_pretty(&root_schema)
}

/// Resolves a character name to a full card path.
///
/// If `name` is empty, defaults to the built-in `Alicia` character.
/// If `name` is a bare name (no path separators), resolves to
/// `{assets}/characters/{name}/character.json`.
/// Otherwise, the name is treated as a literal path.
pub fn resolve_character_path(name: &str) -> String {
    let assets_dir = crate::paths::assets_dir();
    if name.trim().is_empty() {
        format!("{}/characters/Alicia/character.json", assets_dir.display())
    } else if !name.contains('/') && !name.contains('\\') {
        format!(
            "{}/characters/{}/character.json",
            assets_dir.display(),
            name
        )
    } else {
        name.to_string()
    }
}

/// Reads the asset directory and settings.json, resolves character_card_path, etc., and returns EneConfig.
pub fn load_config() -> EneConfig {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_config_from(&assets_dir, &config_path)
}

/// Loads config from the specified asset directory and config file path
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig {
    load_full_config_from(assets_dir, config_path)
}

/// Fully loads the config file. Also auto-updates the schema file on startup
pub fn load_full_config() -> EneConfig {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_config_from(&assets_dir, &config_path)
}

/// Fully loads EneConfig from the specified asset directory and config file path
pub fn load_full_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig {
    use figment::{
        Figment,
        providers::{Env, Format, Json, Serialized},
    };

    let figment = Figment::from(Serialized::defaults(EneConfig::default()))
        .merge(Json::file(config_path))
        .merge(Env::prefixed("ENE_").split("__"));

    let config: EneConfig = figment.extract().unwrap_or_else(|e| {
        tracing::error!("Failed to load configuration: {e}, using default");
        EneConfig::default()
    });

    // Auto-generate schema write-out
    let schema_path = assets_dir.join("settings.schema.json");
    if let Ok(schema_json) = generate_schema_json() {
        let _ = std::fs::write(&schema_path, schema_json);
    }

    // Auto-generate schema write-out for character-specific settings
    let char_schema_path = crate::paths::character_schema_file_path();
    if let Ok(char_schema_json) = crate::character_config::generate_character_schema_json() {
        let _ = std::fs::write(&char_schema_path, char_schema_json);
    }



    update_global_config(config.clone());
    config
}

/// Saves the config file in a type-safe manner
pub fn save_full_config(config: &EneConfig) -> Result<(), std::io::Error> {
    update_global_config(config.clone());
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(config_path, json)?;
    Ok(())
}

/// Loads settings, patches a single section, and saves in one call.
///
/// Convenience wrapper around the load → `set_section` → save pattern.
pub fn update_section<T>(value: &T) -> Result<(), ConfigError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + HasConfigKey,
{
    let mut config = load_config();
    config.set_section(value)?;
    save_full_config(&config).map_err(|e| ConfigError::GenericConfigError(e.to_string()))
}

/// Loads settings, patches a single section by key, and saves in one call.
pub fn update_section_by_key<T>(key: &str, value: &T) -> Result<(), ConfigError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut config = load_config();
    config.set_section_by_key(key, value)?;
    save_full_config(&config).map_err(|e| ConfigError::GenericConfigError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::define_config!(
        "dummy_test_config",
        pub struct DummyTestConfig {
            pub test_value: String = "hello".to_string(),
            pub test_number: i32 = 42,
        }
    );

    crate::define_config!(
        "child_config",
        parent = "ParentDef",
        pub struct ChildTestConfig {
            pub child_field: String = "child".to_string(),
        }
    );

    #[test]
    fn test_define_config_self_registration() {
        let schema_json = generate_schema_json().unwrap();
        assert!(
            schema_json.contains("dummy_test_config"),
            "Schema should automatically include dummy_test_config"
        );
        assert!(
            schema_json.contains("test_value"),
            "Schema should automatically include dummy_test_config field test_value"
        );
    }

    #[test]
    fn test_parent_merge() {
        // Test that merge_child_into_parent merges properties into a definition
        let child_schema = {
            let g = schemars::SchemaGenerator::default();
            g.into_root_schema_for::<ChildTestConfig>()
        };

        let mut parent_schema = {
            let g = schemars::SchemaGenerator::default();
            let mut schema = g.into_root_schema_for::<DummyTestConfig>();
            // Add a dummy ParentDef definition to test against
            let def_schema = schemars::json_schema!({
                "type": "object",
                "properties": {}
            });
            let defs = schema
                .ensure_object()
                .entry("$defs".to_string())
                .or_insert_with(|| serde_json::json!({}));
            defs.as_object_mut()
                .unwrap()
                .insert("ParentDef".to_string(), def_schema.into());
            schema
        };

        merge_child_into_parent(&mut parent_schema, "ParentDef", &child_schema);

        let parent_def = parent_schema
            .get("$defs")
            .and_then(|v| v.get("ParentDef"))
            .unwrap();
        let child_props = parent_def.get("properties").and_then(|v| v.as_object());
        assert!(child_props.is_some(), "ParentDef should have properties");
        assert!(
            child_props.unwrap().contains_key("child_field"),
            "ParentDef should contain child_field"
        );
    }

    #[test]
    fn test_global_config_accessor() {
        let mut raw_config = EneConfig::default();
        raw_config.extra.insert(
            "dummy_test_config".to_string(),
            serde_json::json!({
                "test_value": "custom_val",
                "test_number": 999
            }),
        );
        update_global_config(raw_config);

        let config = get_global_section::<DummyTestConfig>();
        assert_eq!(config.test_value, "custom_val");
        assert_eq!(config.test_number, 999);
    }
}
