use schemars::schema::Schema;
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::Path;
use crate::error::ConfigError;





/// Global singleton holding the active [`EneSettings`].
///
/// Set once at startup by [`load_full_settings`] and updated by
/// [`update_global_settings`]. Accessed by [`get_global_settings`]
/// and [`get_global_section`].
pub static GLOBAL_SETTINGS: std::sync::OnceLock<std::sync::RwLock<EneSettings>> = std::sync::OnceLock::new();

/// Updates the global EneSettings
pub fn update_global_settings(settings: EneSettings) {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = settings;
        }
    } else {
        let _ = GLOBAL_SETTINGS.set(std::sync::RwLock::new(settings));
    }
}

/// Gets a clone of the entire global settings
pub fn get_global_settings() -> EneSettings {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(guard) = lock.read() {
            return guard.clone();
        }
    }
    EneSettings::default()
}

/// Loads a subsection by key from the global settings
pub fn get_global_section<T: serde::de::DeserializeOwned + Default>(key: &str) -> T {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(guard) = lock.read() {
            return guard.get_section::<T>(key).unwrap_or_default();
        }
    }
    T::default()
}

/// A registered config schema entry.
pub struct SchemaEntry {
    /// The JSON Schema definition for this config section.
    pub schema: schemars::schema::RootSchema,
    /// Optional parent key under which this schema should be nested at merge time.
    pub parent: Option<String>,
}

static SCHEMA_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, SchemaEntry>>> = std::sync::OnceLock::new();

/// Registry holding schemas collected from tools at runtime
static RUNTIME_SCHEMA_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, SchemaEntry>>> =
    std::sync::OnceLock::new();

/// Global static helper for each crate to register its own Config schema
pub fn register_schema<T: JsonSchema>(key: &str) {
    register_schema_inner::<T>(key, None);
}

/// Registers schemas collected from tools at runtime
pub fn register_runtime_schema(
    key: &str,
    schema: serde_json::Value,
    parent: Option<String>,
) {
    use schemars::schema::RootSchema;
    let root_schema: RootSchema = serde_json::from_value(schema).unwrap_or_else(|e| {
        tracing::error!("Failed to parse runtime schema for '{}': {}", key, e);
        RootSchema::default()
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
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(key.to_string(), SchemaEntry { schema, parent });
    }
}

fn merge_child_into_parent(
    root_schema: &mut schemars::schema::RootSchema,
    parent_key: &str,
    child_schema: &schemars::schema::RootSchema,
) {
    let child_props = match child_schema.schema.object {
        Some(ref ov) => ov.properties.clone(),
        None => return,
    };
    if child_props.is_empty() {
        return;
    }

    // Try definitions first (e.g. "ToolEntry")
    if let Some(def) = root_schema.definitions.get_mut(parent_key) {
        if let Schema::Object(schema_obj) = def {
            if let Some(ov) = schema_obj.object.as_mut() {
                ov.properties.extend(child_props);
                return;
            }
        }
    }

    // Fall back to root-level property
    if let Some(ov) = root_schema.schema.object.as_mut() {
        if let Some(parent) = ov.properties.get_mut(parent_key) {
            if let Schema::Object(parent_obj) = parent {
                if let Some(parent_ov) = parent_obj.object.as_mut() {
                    parent_ov.properties.extend(child_props);
                }
            }
        }
    }
}

crate::define_config!(
    "ene_settings",
    /// Top-level application settings for ene.
    pub struct EneSettings {
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
        /// Catch-all for provider, tool, and other sub-settings.
        pub extra: HashMap<String, serde_json::Value> = HashMap::new(),
    }
);



impl EneSettings {

    /// Deserialise a sub-section from the `extra` map by key.
    ///
    /// Returns `Ok(T::default())` when the key is absent.
    pub fn get_section<T>(&self, key: &str) -> Result<T, ConfigError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        if let Some(val) = self.extra.get(key) {
            serde_json::from_value(val.clone()).map_err(|e| {
                ConfigError::GenericConfigError(format!("Failed to deserialize section '{}': {}", key, e))
            })
        } else {
            Ok(T::default())
        }
    }

    /// Serialise and insert a sub-section into the `extra` map by key.
    pub fn set_section<T>(&mut self, key: &str, section: &T) -> Result<(), ConfigError>
    where
        T: serde::Serialize,
    {
        let val = serde_json::to_value(section).map_err(|e| {
            ConfigError::GenericConfigError(format!("Failed to serialize section '{}': {}", key, e))
        })?;
        self.extra.insert(key.to_string(), val);
        Ok(())
    }
}


fn apply_registry_to_schema(
    root_schema: &mut schemars::schema::RootSchema,
    registry: &HashMap<String, SchemaEntry>,
) {
    // First pass: insert entries without parents and collect all definitions
    for (key, entry) in registry.iter() {
        if entry.parent.is_none() {
            if let Some(object) = &mut root_schema.schema.object {
                object.properties.insert(
                    key.clone(),
                    schemars::schema::Schema::Object(entry.schema.schema.clone()),
                );
            }
        }
        for (def_name, def_schema) in &entry.schema.definitions {
            root_schema.definitions.insert(def_name.clone(), def_schema.clone());
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
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let mut root_schema = schema_gen.into_root_schema_for::<EneSettings>();
    
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

/// Reads the asset directory and settings.json, resolves character_card_path, etc., and returns EneSettings.
pub fn load_settings() -> EneSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_settings_from(&assets_dir, &config_path)
}

/// Loads settings from the specified asset directory and config file path
pub fn load_settings_from(assets_dir: &Path, config_path: &Path) -> EneSettings {
    load_full_settings_from(assets_dir, config_path)
}

/// Fully loads the config file. Also auto-updates the schema file on startup
pub fn load_full_settings() -> EneSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_settings_from(&assets_dir, &config_path)
}

/// Fully loads EneSettings from the specified asset directory and config file path
pub fn load_full_settings_from(assets_dir: &Path, config_path: &Path) -> EneSettings {
    use figment::{Figment, providers::{Format, Json, Env, Serialized}};

    let figment = Figment::from(Serialized::defaults(EneSettings::default()))
        .merge(Json::file(config_path))
        .merge(Env::prefixed("ENE_").split("__"));

    let mut settings: EneSettings = figment.extract().unwrap_or_else(|e| {
        tracing::error!("Failed to load configuration: {e}, using default");
        EneSettings::default()
    });

    // Auto-generate schema write-out
    let schema_path = assets_dir.join("settings.schema.json");
    if let Ok(schema_json) = generate_schema_json() {
        let _ = std::fs::write(&schema_path, schema_json);
    }

    // Auto-generate schema write-out for character-specific settings
    let char_schema_path = crate::paths::character_schema_file_path();
    if let Ok(char_schema_json) = crate::character_settings::generate_character_schema_json() {
        let _ = std::fs::write(&char_schema_path, char_schema_json);
    }

    if settings.character.trim().is_empty() {
        settings.character = format!("{}/characters/Alicia/character.json", assets_dir.display());
    } else if !settings.character.contains('/') && !settings.character.contains('\\') {
        settings.character = format!(
            "{}/characters/{}/character.json",
            assets_dir.display(),
            settings.character
        );
    }

    update_global_settings(settings.clone());
    settings
}

/// Saves the config file in a type-safe manner
pub fn save_full_settings(settings: &EneSettings) -> Result<(), std::io::Error> {
    update_global_settings(settings.clone());
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(config_path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::schema::Schema;

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
        assert!(schema_json.contains("dummy_test_config"), "Schema should automatically include dummy_test_config");
        assert!(schema_json.contains("test_value"), "Schema should automatically include dummy_test_config field test_value");
    }

    #[test]
    fn test_parent_merge() {
        // Test that merge_child_into_parent merges properties into a definition
        let child_schema = {
            let g = schemars::r#gen::SchemaSettings::draft07().into_generator();
            g.into_root_schema_for::<ChildTestConfig>()
        };

        let mut parent_schema = {
            let g = schemars::r#gen::SchemaSettings::draft07().into_generator();
            let mut schema = g.into_root_schema_for::<DummyTestConfig>();
            // Add a dummy ParentDef definition to test against
            let def_schema = Schema::Object(schemars::schema::SchemaObject {
                object: Some(Box::new(schemars::schema::ObjectValidation::default())),
                ..Default::default()
            });
            schema.definitions.insert("ParentDef".to_string(), def_schema);
            schema
        };

        merge_child_into_parent(&mut parent_schema, "ParentDef", &child_schema);

        let parent_def = parent_schema.definitions.get("ParentDef").unwrap();
        let child_props = match parent_def {
            Schema::Object(obj) => obj.object.as_ref().map(|o| &o.properties),
            _ => None,
        };
        assert!(child_props.is_some(), "ParentDef should have properties");
        assert!(child_props.unwrap().contains_key("child_field"), "ParentDef should contain child_field");
    }

    #[test]
    fn test_global_settings_accessor() {
        let mut raw_settings = EneSettings::default();
        raw_settings.extra.insert(
            "dummy_test_config".to_string(),
            serde_json::json!({
                "test_value": "custom_val",
                "test_number": 999
            })
        );
        update_global_settings(raw_settings);

        let config = get_global_section::<DummyTestConfig>("dummy_test_config");
        assert_eq!(config.test_value, "custom_val");
        assert_eq!(config.test_number, 999);
    }
}
