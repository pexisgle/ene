use crate::error::EneConfigError;
use schemars::JsonSchema;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Global singleton holding the active [`EneConfig`].
pub static GLOBAL_CONFIG: std::sync::OnceLock<std::sync::RwLock<EneConfig>> =
    std::sync::OnceLock::new();

/// Updates the global `EneConfig`
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
    if let Some(lock) = GLOBAL_CONFIG.get()
        && let Ok(guard) = lock.read()
    {
        return guard.clone();
    }
    EneConfig::default()
}

/// Trait for config structs that possess a unique config key.
pub trait HasConfigKey {
    /// The string key of this configuration section under its parent.
    const KEY: &'static str;

    /// The target configuration file (Settings or Character).
    const TARGET: ConfigTarget;

    /// The full path from the root.
    fn path() -> &'static [&'static str];
}

/// Loads a subsection from the global config using the type's associated key and path.
pub fn get_global_section<T>() -> T
where
    T: serde::de::DeserializeOwned + Default + HasConfigKey,
{
    if let Some(lock) = GLOBAL_CONFIG.get()
        && let Ok(guard) = lock.read()
    {
        return guard.get_section::<T>().unwrap_or_default();
    }
    T::default()
}

/// The target of the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTarget {
    /// Config belongs to settings.json
    Settings,
    /// Config belongs to character_settings.json
    Character,
}

/// A registered config schema entry.
pub struct SchemaEntry {
    /// The JSON Schema definition.
    pub schema: schemars::Schema,
    /// Target file.
    pub target: ConfigTarget,
    /// Parent key.
    pub parent_key: Option<String>,
}

static SCHEMA_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, SchemaEntry>>> =
    std::sync::OnceLock::new();

/// Registers schemas collected from tools or compile-time config structs
#[doc(hidden)]
pub fn __register_schema<T: JsonSchema + HasConfigKey>(
    target: ConfigTarget,
    parent_key: Option<&str>,
) {
    let schema_gen = schemars::SchemaGenerator::default();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(
            T::KEY.to_string(),
            SchemaEntry {
                schema,
                target,
                parent_key: parent_key.map(String::from),
            },
        );
    }
}

/// Tool schema registration helper
#[doc(hidden)]
pub fn __register_tool_schema<T: JsonSchema>(tool_name: &str) {
    let schema_gen = schemars::SchemaGenerator::default();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(
            tool_name.to_string(),
            SchemaEntry {
                schema,
                target: ConfigTarget::Settings,
                parent_key: Some("tools_map".to_string()),
            },
        );
    }
}

/// Registers schemas collected at runtime
pub fn register_runtime_schema(key: &str, schema: serde_json::Value) {
    let root_schema: schemars::Schema = serde_json::from_value(schema).unwrap_or_else(|e| {
        tracing::error!("Failed to parse runtime schema for '{}': {}", key, e);
        schemars::Schema::default()
    });
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(
            key.to_string(),
            SchemaEntry {
                schema: root_schema,
                target: ConfigTarget::Settings,
                parent_key: None,
            },
        );
    }
}

/// Top-level settings configuration for the Ene platform.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct EneConfig {
    /// Schema version number.
    pub version: u32,
    /// Character card name or path.
    pub character: String,
    /// Display name shown to the user.
    pub user_name: String,
    /// Behavioural rules injected into every system prompt.
    pub runtime_rules: String,

    #[serde(flatten)]
    #[schemars(skip)]
    /// Catch-all for provider, tool, and other sub-configurations.
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for EneConfig {
    fn default() -> Self {
        Self {
            version: 1,
            character: String::new(),
            user_name: "User".to_string(),
            runtime_rules: "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.".to_string(),
            extra: BTreeMap::new(),
        }
    }
}

impl EneConfig {
    /// Deserialise a sub-section from the `extra` map using the type's associated path.
    ///
    /// Returns `Ok(T::default())` when the key/path is absent.
    pub fn get_section<T>(&self) -> Result<T, EneConfigError>
    where
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Settings);
        let mut cur = serde_json::Value::Object(
            self.extra
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        for key in T::path() {
            match cur.get(key).cloned() {
                Some(v) => cur = v,
                None => return Ok(T::default()),
            }
        }
        serde_json::from_value(cur).map_err(|e| {
            EneConfigError::GenericConfigError(format!("Failed to deserialize nested section: {e}"))
        })
    }

    /// Serialise and insert a sub-section into the `extra` map using the type's associated path.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Settings);
        let val = serde_json::to_value(section).map_err(|e| {
            EneConfigError::GenericConfigError(format!("Failed to serialize section: {e}"))
        })?;
        set_nested(&mut self.extra, T::path(), val)?;
        Ok(())
    }
}

fn set_nested(
    extra: &mut BTreeMap<String, serde_json::Value>,
    path: &[&str],
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    if path.is_empty() {
        return Err(EneConfigError::GenericConfigError(
            "Empty path for nested config".to_string(),
        ));
    }

    let mut root =
        serde_json::Value::Object(extra.iter().map(|(k, v)| (k.clone(), v.clone())).collect());

    let mut cur = &mut root;
    for (i, &key) in path.iter().enumerate() {
        if i == path.len() - 1 {
            if let Some(obj) = cur.as_object_mut() {
                obj.insert(key.to_string(), value);
            }
            break;
        } else {
            if !cur.is_object() {
                *cur = serde_json::Value::Object(serde_json::Map::new());
            }
            let obj = cur.as_object_mut().unwrap();
            cur = obj
                .entry(key.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        }
    }

    if let serde_json::Value::Object(obj) = root {
        *extra = obj.into_iter().collect();
    }

    Ok(())
}

/// Generates the JSON representation of the JSON Schema for settings.json
pub fn generate_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<EneConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(registry) = SCHEMA_REGISTRY.get()
        && let Ok(reg) = registry.lock()
        && let Some(root_obj) = root_val.as_object_mut()
    {
        // 1. Copy definitions
        for entry in reg.values() {
            if entry.target != ConfigTarget::Settings {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;
            if let Some(definitions) = entry_val
                .get("definitions")
                .or_else(|| entry_val.get("$defs"))
                .and_then(|v| v.as_object())
            {
                let root_defs = root_obj
                    .entry("definitions".to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .unwrap();
                for (def_name, def_schema) in definitions {
                    root_defs.insert(def_name.clone(), def_schema.clone());
                }
            }
        }

        // 2. Add properties
        for (key, entry) in reg.iter() {
            if entry.target != ConfigTarget::Settings {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;

            if let Some(parent_key) = &entry.parent_key {
                if parent_key == "tools_map" {
                    let tool_config_def = if root_obj.contains_key("definitions") {
                        root_obj
                            .get_mut("definitions")
                            .and_then(|d| d.get_mut("ToolConfig"))
                    } else {
                        root_obj
                            .get_mut("$defs")
                            .and_then(|d| d.get_mut("ToolConfig"))
                    };
                    if let Some(tool_config_def) = tool_config_def
                        && let Some(tools_prop) = tool_config_def
                            .get_mut("properties")
                            .and_then(|p| p.get_mut("tools"))
                    {
                        let tools_obj = tools_prop.as_object_mut().unwrap();
                        let properties = tools_obj
                            .entry("properties".to_string())
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                            .unwrap();

                        let mut clean_entry = entry_val.clone();
                        if let Some(obj) = clean_entry.as_object_mut() {
                            obj.remove("definitions");
                            obj.remove("$schema");
                        }
                        properties.insert(
                            key.clone(),
                            serde_json::json!({
                                "allOf": [
                                    { "$ref": "#/definitions/ToolEntry" },
                                    clean_entry
                                ]
                            }),
                        );
                    }
                }
            } else {
                let properties = root_obj
                    .entry("properties".to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .unwrap();
                let mut clean_entry = entry_val.clone();
                if let Some(obj) = clean_entry.as_object_mut() {
                    obj.remove("definitions");
                    obj.remove("$schema");
                }
                properties.insert(key.clone(), clean_entry);
            }
        }
    }

    let root_schema: schemars::Schema = serde_json::from_value(root_val)?;
    serde_json::to_string_pretty(&root_schema)
}

/// Generates the JSON representation of the JSON Schema for character_settings.json
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<crate::character_config::CharacterConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(registry) = SCHEMA_REGISTRY.get()
        && let Ok(reg) = registry.lock()
        && let Some(root_obj) = root_val.as_object_mut()
    {
        // 1. Copy definitions
        for entry in reg.values() {
            if entry.target != ConfigTarget::Character {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;
            if let Some(definitions) = entry_val
                .get("definitions")
                .or_else(|| entry_val.get("$defs"))
                .and_then(|v| v.as_object())
            {
                let root_defs = root_obj
                    .entry("definitions".to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
                    .unwrap();
                for (def_name, def_schema) in definitions {
                    root_defs.insert(def_name.clone(), def_schema.clone());
                }
            }
        }

        // 2. Add properties
        for (key, entry) in reg.iter() {
            if entry.target != ConfigTarget::Character {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;
            let properties = root_obj
                .entry("properties".to_string())
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .unwrap();
            let mut clean_entry = entry_val.clone();
            if let Some(obj) = clean_entry.as_object_mut() {
                obj.remove("definitions");
                obj.remove("$schema");
            }
            properties.insert(key.clone(), clean_entry);
        }
    }

    let root_schema: schemars::Schema = serde_json::from_value(root_val)?;
    serde_json::to_string_pretty(&root_schema)
}

/// Generates the JSON representation of the JSON Schema for character.json (CharacterCardV3)
pub fn generate_character_card_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<crate::character_card::CharacterCardV3>();
    serde_json::to_string_pretty(&root_schema)
}

/// Resolves a character name to a full card path.
#[must_use]
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

/// Reads the asset directory and settings.json, resolves `character_card_path`, etc., and returns `EneConfig`.
#[must_use]
pub fn load_config() -> EneConfig {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_config_from(&assets_dir, &config_path)
}

/// Loads config from the specified asset directory and config file path
#[must_use]
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig {
    load_full_config_from(assets_dir, config_path)
}

/// Fully loads the config file. Also auto-updates the schema file on startup
#[must_use]
pub fn load_full_config() -> EneConfig {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_config_from(&assets_dir, &config_path)
}

/// Fully loads `EneConfig` from the specified asset directory and config file path
#[must_use]
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

    // Ensure schema directory exists
    let schema_dir = assets_dir.join("schema");
    let _ = std::fs::create_dir_all(&schema_dir);

    write_schemas(assets_dir);

    update_global_config(config.clone());
    config
}

/// Auto-generates and writes out settings and character schemas under the assets schema directory.
pub fn write_schemas(assets_dir: &Path) {
    let _ = std::fs::create_dir_all(assets_dir.join("schema"));

    // Auto-generate schema write-out
    let schema_path = crate::paths::schema_file_path();
    if let Ok(schema_json) = generate_schema_json() {
        let _ = std::fs::write(&schema_path, schema_json);
    }

    // Auto-generate schema write-out for character-specific settings
    let char_schema_path = crate::paths::character_schema_file_path();
    if let Ok(char_schema_json) = generate_character_schema_json() {
        let _ = std::fs::write(&char_schema_path, char_schema_json);
    }

    // Auto-generate schema write-out for character card (character.json)
    let char_card_schema_path = crate::paths::character_card_schema_file_path();
    if let Ok(char_card_schema_json) = generate_character_card_schema_json() {
        let _ = std::fs::write(&char_card_schema_path, char_card_schema_json);
    }
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
pub fn update_section<T>(value: &T) -> Result<(), EneConfigError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + HasConfigKey,
{
    let mut config = load_config();
    config.set_section(value)?;
    save_full_config(&config).map_err(|e| EneConfigError::GenericConfigError(e.to_string()))
}
