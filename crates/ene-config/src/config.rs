use crate::character_card::UserPersona;
use crate::error::EneConfigError;
use schemars::JsonSchema;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Global singleton holding the active [`EneConfig`].
///
/// Uses `parking_lot::RwLock` which does not poison on panic, matching the
/// `ConfigStore` lock strategy.
pub static GLOBAL_CONFIG: std::sync::OnceLock<parking_lot::RwLock<EneConfig>> =
    std::sync::OnceLock::new();

/// Updates the global `EneConfig`
pub fn update_global_config(config: EneConfig) {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        *lock.write() = config;
    } else {
        // If another thread raced us and set it first, that write already
        // landed, so a failed `set` here is a no-op we can safely discard.
        drop(GLOBAL_CONFIG.set(parking_lot::RwLock::new(config)));
    }
}

/// Gets a clone of the entire global config
pub fn get_global_config() -> EneConfig {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        return lock.read().clone();
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
    if let Some(lock) = GLOBAL_CONFIG.get() {
        return lock.read().get_section::<T>().unwrap_or_default();
    }
    T::default()
}

/// The target of the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigTarget {
    /// Config belongs to settings.json
    Settings,
    /// Config belongs to `character_settings.json`
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

static SCHEMA_REGISTRY: std::sync::OnceLock<parking_lot::Mutex<HashMap<String, SchemaEntry>>> =
    std::sync::OnceLock::new();

/// Registers schemas collected from tools or compile-time config structs
#[doc(hidden)]
pub fn register_config_schema<T: JsonSchema + HasConfigKey>(
    target: ConfigTarget,
    parent_key: Option<&str>,
) {
    let schema_gen = schemars::SchemaGenerator::default();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let mut reg = registry.lock();
    reg.insert(
        T::KEY.to_string(),
        SchemaEntry {
            schema,
            target,
            parent_key: parent_key.map(String::from),
        },
    );
}

/// Tool schema registration helper
#[doc(hidden)]
pub fn register_tool_schema<T: JsonSchema>(tool_name: &str) {
    let schema_gen = schemars::SchemaGenerator::default();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let mut reg = registry.lock();
    reg.insert(
        tool_name.to_string(),
        SchemaEntry {
            schema,
            target: ConfigTarget::Settings,
            parent_key: Some("tools_map".to_string()),
        },
    );
}

/// Registers schemas collected at runtime.
///
/// Returns `Err` if the JSON value cannot be parsed as a valid JSON Schema.
pub fn register_runtime_schema(key: &str, schema: serde_json::Value) -> Result<(), EneConfigError> {
    let root_schema: schemars::Schema = serde_json::from_value(schema).map_err(|e| {
        EneConfigError::GenericConfigError(format!(
            "Failed to parse runtime schema for '{key}': {e}"
        ))
    })?;
    let registry = SCHEMA_REGISTRY.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let mut reg = registry.lock();
    reg.insert(
        key.to_string(),
        SchemaEntry {
            schema: root_schema,
            target: ConfigTarget::Settings,
            parent_key: None,
        },
    );
    Ok(())
}

/// Default overlay-oriented behavioural rules when none are configured.
pub const DEFAULT_RUNTIME_RULES: &str =
    "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.";

fn runtime_rules_is_default(rules: &str) -> bool {
    rules.is_empty() || rules == DEFAULT_RUNTIME_RULES
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
    #[serde(default, skip_serializing_if = "runtime_rules_is_default")]
    pub runtime_rules: String,

    /// Optional structured user persona for roleplay context.
    /// When set, the `{{user_persona}}` CBS macro expands to this persona's fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_persona: Option<UserPersona>,

    #[serde(flatten)]
    #[schemars(skip)]
    /// Catch-all for provider, tool, and other sub-configurations.
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for EneConfig {
    fn default() -> Self {
        Self {
            version: 1,
            character: "Alicia".to_string(),
            user_name: "User".to_string(),
            runtime_rules: DEFAULT_RUNTIME_RULES.to_string(),
            user_persona: None,
            extra: BTreeMap::new(),
        }
    }
}

impl EneConfig {
    /// Deserialise a sub-section from the `extra` map using the type's associated path.
    ///
    /// Returns `Ok(T::default())` when the key/path is absent.
    ///
    /// Refuses types whose `TARGET` is `Character`; those
    /// sections live in `CharacterConfig::extra` and must
    /// go through [`CharacterConfig::get_section`]. The
    /// previous `debug_assert` silently read from the wrong
    /// map in release builds.
    pub fn get_section<T>(&self) -> Result<T, EneConfigError>
    where
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        if T::TARGET != ConfigTarget::Settings {
            return Err(EneConfigError::GenericConfigError(format!(
                "`{}` is a Character-target section; use CharacterConfig::get_section instead",
                T::KEY
            )));
        }
        // Walk the path directly through the
        // BTreeMap, descending into nested objects one
        // level at a time. The previous form rebuilt
        // the entire `extra` map into a JSON object
        // on every call (O(n) per read) and required
        // cloning every value.
        let mut current: Option<&serde_json::Value> = None;
        for (i, key) in T::path().iter().enumerate() {
            if i == 0 {
                match self.extra.get(*key) {
                    Some(v) => current = Some(v),
                    None => return Ok(T::default()),
                }
                continue;
            }
            let Some(cur_val) = current else {
                return Ok(T::default());
            };
            match cur_val.as_object().and_then(|o| o.get(*key)) {
                Some(v) => current = Some(v),
                None => return Ok(T::default()),
            }
        }
        let Some(final_val) = current else {
            return Ok(T::default());
        };
        serde_json::from_value(final_val.clone()).map_err(|e| {
            EneConfigError::GenericConfigError(format!("Failed to deserialize nested section: {e}"))
        })
    }

    /// Serialise and insert a sub-section into the `extra` map using the type's associated path.
    ///
    /// Refuses types whose `TARGET` is `Character`; those
    /// sections live in `CharacterConfig::extra` and must
    /// go through [`CharacterConfig::set_section`]. The
    /// previous `debug_assert` silently wrote to the wrong
    /// map in release builds.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        if T::TARGET != ConfigTarget::Settings {
            return Err(EneConfigError::GenericConfigError(format!(
                "`{}` is a Character-target section; use CharacterConfig::set_section instead",
                T::KEY
            )));
        }
        let val = serde_json::to_value(section).map_err(|e| {
            EneConfigError::GenericConfigError(format!("Failed to serialize section: {e}"))
        })?;
        // Skip the write if the serialised value is already identical
        // to what sits at this path. This avoids redundant BTreeMap
        // mutations and prevents unnecessary dirty-flag flips.
        let path = T::path();
        let mut current: Option<&serde_json::Value> = None;
        for (i, key) in path.iter().enumerate() {
            if i == 0 {
                current = self.extra.get(*key);
            } else {
                current = current
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get(*key));
            }
        }
        if current.is_some_and(|existing| *existing == val) {
            return Ok(());
        }
        set_nested(&mut self.extra, path, val)?;
        Ok(())
    }

    /// Set a value at a dotted JSON path under `extra` (e.g. `ai.tasks.chat.model`).
    ///
    /// `value` is parsed as JSON when possible; otherwise treated as a string.
    /// Used by CLI `/config set` (#241).
    pub fn set_path(&mut self, dotted_path: &str, raw_value: &str) -> Result<(), EneConfigError> {
        let path: Vec<&str> = dotted_path
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if path.is_empty() {
            return Err(EneConfigError::GenericConfigError(
                "empty config path".to_string(),
            ));
        }
        let value = match serde_json::from_str::<serde_json::Value>(raw_value) {
            Ok(v) => v,
            Err(_) => serde_json::Value::String(raw_value.to_string()),
        };
        set_nested(&mut self.extra, &path, value)
    }

    /// Read a value at a dotted JSON path under `extra` (#241).
    ///
    /// Walks the `BTreeMap` directly instead of serialising the entire `extra`
    /// map into a JSON `Value` tree.
    pub fn get_path(&self, dotted_path: &str) -> Option<serde_json::Value> {
        let keys: Vec<&str> = dotted_path.split('.').filter(|s| !s.is_empty()).collect();
        if keys.is_empty() {
            return None;
        }
        let mut current: Option<&serde_json::Value> = None;
        for (i, key) in keys.iter().enumerate() {
            if i == 0 {
                current = Some(self.extra.get(*key)?);
                continue;
            }
            current = Some(current?.as_object()?.get(*key)?);
        }
        current.cloned()
    }
}

pub(crate) fn set_nested(
    extra: &mut BTreeMap<String, serde_json::Value>,
    path: &[&str],
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    // Descend through the BTreeMap, mutating the path
    // in place. The previous form rebuilt the entire
    // `extra` map into a JSON object (O(n) on every
    // write) and silently dropped the write if `cur`
    // ever landed on a non-object leaf.
    let Some((head, rest)) = path.split_first() else {
        return Err(EneConfigError::GenericConfigError(
            "Empty path for nested config".to_string(),
        ));
    };
    if rest.is_empty() {
        extra.insert((*head).to_string(), value);
        return Ok(());
    }

    let mut current: &mut serde_json::Value = extra
        .entry((*head).to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    for (i, key) in rest.iter().enumerate() {
        let is_last = i.saturating_add(1) == rest.len();
        if is_last {
            // The final key may either replace an
            // existing value or be inserted as a new
            // entry. If the existing value at this path
            // is a non-object leaf (e.g. a string),
            // surface a typed error rather than
            // silently overwriting it with a nested
            // structure.
            if let Some(existing) = current.as_object().and_then(|o| o.get(*key)) {
                if !existing.is_object() && !value.is_object() {
                    // Both leaves: replace is fine.
                } else if !existing.is_object() {
                    return Err(EneConfigError::GenericConfigError(format!(
                        "set_nested: cannot insert nested value at path \
                         `{}`; existing value is a non-object leaf ({})",
                        path.join("."),
                        existing
                    )));
                }
            }
            let obj = current.as_object_mut().ok_or_else(|| {
                EneConfigError::GenericConfigError(format!(
                    "set_nested: cannot descend into non-object at path `{}`",
                    path.join(".")
                ))
            })?;
            obj.insert((*key).to_string(), value);
            return Ok(());
        }

        // Intermediate key: ensure the value is an
        // object so we can descend. If a non-object
        // leaf sits in the middle of the path, surface
        // a typed error rather than silently replacing
        // it with a fresh object.
        if let Some(existing) = current.as_object().and_then(|o| o.get(*key))
            && !existing.is_object()
        {
            return Err(EneConfigError::GenericConfigError(format!(
                "set_nested: cannot descend through non-object leaf at \
                 path `{}` (existing: {})",
                path.join("."),
                existing
            )));
        }
        let obj = current.as_object_mut().ok_or_else(|| {
            EneConfigError::GenericConfigError(format!(
                "set_nested: cannot descend into non-object at path `{}`",
                path.join(".")
            ))
        })?;
        current = obj
            .entry((*key).to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }

    Ok(())
}

/// Generates the JSON representation of the JSON Schema for settings.json
pub fn generate_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<EneConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(registry) = SCHEMA_REGISTRY.get()
        && let Some(root_obj) = root_val.as_object_mut()
    {
        let reg = registry.lock();
        // 1. Copy definitions
        for entry in reg.values() {
            if entry.target != ConfigTarget::Settings {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;
            let def_key = if root_obj.contains_key("$defs") {
                "$defs"
            } else {
                "definitions"
            };
            if let Some(definitions) = entry_val
                .get("$defs")
                .or_else(|| entry_val.get("definitions"))
                .and_then(|v| v.as_object())
                && let Some(root_defs) = root_obj
                    .entry(def_key.to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
            {
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
                        && let Some(props) = tool_config_def
                            .get_mut("properties")
                            .and_then(|p| p.as_object_mut())
                    {
                        let map_key = if props.contains_key("list") {
                            "list"
                        } else if props.contains_key("tools") {
                            "tools"
                        } else {
                            ""
                        };
                        if !map_key.is_empty()
                            && let Some(tools_prop) = props.get_mut(map_key)
                            && let Some(tools_obj) = tools_prop.as_object_mut()
                            && let Some(properties) = tools_obj
                                .entry("properties".to_string())
                                .or_insert_with(|| serde_json::json!({}))
                                .as_object_mut()
                        {
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
                } else if parent_key == "tools" {
                    // Nested under `tools.*` (e.g. `tools.rag`), sibling of `list`.
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
                        && let Some(properties) = tool_config_def
                            .get_mut("properties")
                            .and_then(|p| p.as_object_mut())
                    {
                        let mut clean_entry = entry_val.clone();
                        if let Some(obj) = clean_entry.as_object_mut() {
                            obj.remove("definitions");
                            obj.remove("$schema");
                        }
                        properties.insert(key.clone(), clean_entry);
                    }
                }
            } else if let Some(properties) = root_obj
                .entry("properties".to_string())
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
            {
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

/// Generates the JSON representation of the JSON Schema for `character_settings.json`
// TODO(M8): `generate_schema_json` and `generate_character_schema_json` share ~80%
// identical code (root schema generation, definition copying, property injection).
// Extract the shared logic into a single parameterised function that accepts a
// `ConfigTarget` filter and a closure for the special `tools_map`/`tools` handling.
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<crate::character_config::CharacterConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(registry) = SCHEMA_REGISTRY.get()
        && let Some(root_obj) = root_val.as_object_mut()
    {
        let reg = registry.lock();
        // 1. Copy definitions
        for entry in reg.values() {
            if entry.target != ConfigTarget::Character {
                continue;
            }
            let entry_val = serde_json::to_value(&entry.schema)?;
            let def_key = if root_obj.contains_key("$defs") {
                "$defs"
            } else {
                "definitions"
            };
            if let Some(definitions) = entry_val
                .get("$defs")
                .or_else(|| entry_val.get("definitions"))
                .and_then(|v| v.as_object())
                && let Some(root_defs) = root_obj
                    .entry(def_key.to_string())
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut()
            {
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
            if let Some(properties) = root_obj
                .entry("properties".to_string())
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
            {
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

/// Generates the JSON representation of the JSON Schema for character.json (`CharacterCardV3`)
pub fn generate_character_card_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<crate::character_card::CharacterCardV3>();
    serde_json::to_string_pretty(&root_schema)
}

/// Resolves a character name to a full card path.
#[must_use]
pub fn resolve_character_path(name: &str) -> PathBuf {
    let assets_dir = crate::paths::assets_dir();
    if name.trim().is_empty() {
        assets_dir
            .join("characters")
            .join("Alicia")
            .join("character.json")
    } else if !name.contains('/') && !name.contains('\\') {
        assets_dir
            .join("characters")
            .join(name)
            .join("character.json")
    } else {
        PathBuf::from(name)
    }
}

/// Loads a [`CharacterCardV3`] from a resolved path (or bare character name).
///
/// Host apps (`ene-cli`, `ene-desktop`) load the card via this helper (or their
/// own I/O) and pass it to [`ene_runtime::EneHandle::open`] — the runtime does
/// not perform character-card file I/O on the product path.
pub fn load_character_card(
    name_or_path: &str,
) -> Result<crate::CharacterCardV3, crate::EneConfigError> {
    let path = resolve_character_path(name_or_path);
    let file_content =
        std::fs::read_to_string(&path).map_err(crate::EneConfigError::CardReadError)?;
    serde_json::from_str(&file_content).map_err(crate::EneConfigError::JsonError)
}

/// Reads the asset directory and settings.json, resolves `character_card_path`, etc., and returns `EneConfig`.
///
/// Returns [`EneConfigError`] if the on-disk `settings.json` is malformed,
/// env-var parsing fails, or required fields cannot be deserialised. This
/// is a breaking change from the previous behavior, which silently
/// reset the entire config to defaults on any extract failure.
pub fn load_config() -> Result<EneConfig, EneConfigError> {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_config_from(assets_dir, &config_path)
}

/// Loads config from the specified asset directory and config file path.
///
/// Returns [`EneConfigError`] on any extract failure. See [`load_config`].
pub fn load_config_from(
    assets_dir: &Path,
    config_path: &Path,
) -> Result<EneConfig, EneConfigError> {
    load_full_config_from(assets_dir, config_path)
}

/// Fully loads the config file.
///
/// Returns [`EneConfigError`] on any extract failure. See [`load_config`].
pub fn load_full_config() -> Result<EneConfig, EneConfigError> {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_config_from(assets_dir, &config_path)
}

/// Fully loads `EneConfig` from the specified asset directory and config file path.
///
/// Returns [`EneConfigError`] on any extract failure. See [`load_config`].
///
/// # Env-var case folding
///
/// The `ENE_` env provider applies `.map(|k| k.to_lowercase())` so that
/// `ENE_AI__TASKS__CHAT__MODEL` resolves to the `ai.tasks.chat.model` path
/// on [`EneConfig`] (lowercase). Without the case-folding, Figment stored
/// the path as `AI.tasks.chat.model` and the value was silently dropped
/// because `get_section::<AiConfig>()` looks up `T::path() = ["ai"]`
/// (lowercase).
pub fn load_full_config_from(
    _assets_dir: &Path,
    config_path: &Path,
) -> Result<EneConfig, EneConfigError> {
    use figment::{
        Figment,
        providers::{Env, Format, Json, Serialized},
    };

    let figment = Figment::from(Serialized::defaults(EneConfig::default()))
        .merge(Json::file(config_path))
        // `.map(...)` makes env vars case-insensitive against the
        // lowercase config keys, matching the documented
        // `ENE_AI__TASKS__CHAT__MODEL` examples.
        .merge(
            Env::prefixed("ENE_")
                .split("__")
                .map(|k| k.as_str().to_lowercase().into()),
        );

    let config: EneConfig = figment.extract().map_err(|e| {
        EneConfigError::GenericConfigError(format!("configuration extract failed: {e}"))
    })?;

    update_global_config(config.clone());
    Ok(config)
}

/// Auto-generates and writes out settings and character schemas under the assets schema directory.
pub fn write_schemas(assets_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(assets_dir.join("schema")) {
        tracing::error!(component = "Config", error = %e, "Failed to create schema directory");
        return;
    }

    let schema_path = crate::paths::schema_file_path();
    if let Ok(schema_json) = generate_schema_json()
        && let Err(e) = std::fs::write(&schema_path, schema_json)
    {
        tracing::error!(component = "Config", path = %schema_path.display(), error = %e, "Failed to write settings schema");
    }

    let char_schema_path = crate::paths::character_schema_file_path();
    if let Ok(char_schema_json) = generate_character_schema_json()
        && let Err(e) = std::fs::write(&char_schema_path, char_schema_json)
    {
        tracing::error!(component = "Config", path = %char_schema_path.display(), error = %e, "Failed to write character schema");
    }

    let char_card_schema_path = crate::paths::character_card_schema_file_path();
    if let Ok(char_card_schema_json) = generate_character_card_schema_json()
        && let Err(e) = std::fs::write(&char_card_schema_path, char_card_schema_json)
    {
        tracing::error!(component = "Config", path = %char_card_schema_path.display(), error = %e, "Failed to write character card schema");
    }
}

/// Atomically writes `contents` to `path` by first writing to a temporary
/// file in the same directory, then renaming over the target.
///
/// The rename is atomic on POSIX when source and destination reside on the
/// same filesystem, which is guaranteed by placing the temp file in the
/// target's parent directory. This prevents partial or corrupt config files
/// if the process crashes mid-write (#325).
pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<(), EneConfigError> {
    use std::io::Write;

    let dir = path.parent().ok_or_else(|| {
        EneConfigError::GenericConfigError(format!("no parent directory for {}", path.display()))
    })?;
    std::fs::create_dir_all(dir).map_err(EneConfigError::IoError)?;

    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("config");
    let tmp_path = dir.join(format!(".{file_name}.tmp"));

    let mut file = std::fs::File::create(&tmp_path).map_err(EneConfigError::IoError)?;
    file.write_all(contents.as_bytes())
        .map_err(EneConfigError::IoError)?;

    // Best-effort fsync: ensure bytes reach stable storage before the
    // rename makes them visible. Failure is non-fatal — the rename is
    // still atomic, we just lose the durability guarantee on exotic
    // filesystems.
    if let Err(e) = file.sync_all() {
        tracing::debug!(
            component = "Config",
            path = %tmp_path.display(),
            error = %e,
            "best-effort fsync before rename failed (non-fatal)"
        );
    }

    drop(file);
    std::fs::rename(&tmp_path, path).map_err(EneConfigError::IoError)?;
    Ok(())
}

/// Saves the config file in a type-safe manner, using an atomic
/// temp-file-then-rename strategy to avoid partial writes (#325).
pub fn save_full_config(config: &EneConfig) -> Result<(), EneConfigError> {
    update_global_config(config.clone());
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(EneConfigError::IoError)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    atomic_write(&config_path, &json)?;
    Ok(())
}

/// Loads settings, patches a single section, and saves in one call.
pub fn update_section<T>(value: &T) -> Result<(), EneConfigError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + HasConfigKey,
{
    let mut config = load_config()?;
    config.set_section(value)?;
    save_full_config(&config)
}

#[cfg(test)]
#[expect(
    clippy::undocumented_unsafe_blocks,
    reason = "test-only set_var/remove_var under a process-global mutex"
)]
mod tests {
    use super::*;
    use figment::{
        Figment,
        providers::{Env, Format, Json, Serialized},
    };
    use std::sync::Mutex;

    /// env-var tests in this module call `set_var`, which is process-global
    /// and panics if invoked concurrently from multiple threads. A static
    /// mutex serializes them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Direct re-implementation of the `load_full_config_from` env-var
    /// merging logic, but with `assets_dir` and `config_path` injected
    /// rather than read from the global paths, so we can test the
    /// env-var folding in isolation.
    fn figment_with_settings_json(config_path: &Path) -> Figment {
        Figment::from(Serialized::defaults(EneConfig::default()))
            .merge(Json::file(config_path))
            .merge(
                Env::prefixed("ENE_TEST_")
                    .split("__")
                    .map(|k| k.as_str().to_lowercase().into()),
            )
    }

    /// Inspect the env-var-derived `extra` map directly, instead of
    /// going through `get_section::<T>()`. This avoids the dual-crate
    /// problem (`ene_ai` is not a dev-dep of `ene_config`, so its
    /// `define_config!`-generated impls of `HasConfigKey` are for a
    /// different copy of the trait). The `extra` map is what
    /// `get_section` reads from, so checking it is equivalent to
    /// checking the env-var folding.
    fn extra_keys(cfg: &EneConfig) -> Vec<String> {
        cfg.extra.keys().cloned().collect()
    }

    /// Regression for #40: the case-folding `.map(|k| k.to_lowercase())`
    /// must turn `ENE_TEST_PROVIDER__API_KEY` into the lowercase
    /// `provider.api_key` path. Pre-fix, the path was stored as
    /// `PROVIDER.api_key` and section lookups under the lowercase key
    /// silently got nothing. (Same folding applies to `ENE_AI__…` paths.)
    #[test]
    fn env_uppercase_folds_to_lowercase_path() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: serialized by ENV_LOCK; no other threads touch this env var.
        unsafe {
            std::env::set_var("ENE_TEST_PROVIDER__API_KEY", "sk-test-1234");
        }
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("write empty settings fixture");

        let fig = figment_with_settings_json(&path);
        let cfg: EneConfig = fig.extract().expect("empty settings extracts defaults");

        unsafe {
            std::env::remove_var("ENE_TEST_PROVIDER__API_KEY");
        }

        let keys = extra_keys(&cfg);
        assert!(
            keys.contains(&"provider".to_string()),
            "expected lowercase 'provider' key in extra, got {keys:?}"
        );
        assert!(
            !keys.contains(&"PROVIDER".to_string()),
            "uppercase 'PROVIDER' key should have been folded to lowercase, got {keys:?}"
        );
    }

    /// Lowercase env vars must also work — case-folding is
    /// idempotent for already-lowercase input.
    #[test]
    fn env_lowercase_works() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("ENE_TEST_provider__api_key", "sk-lowercase");
        }
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("write empty settings fixture");

        let fig = figment_with_settings_json(&path);
        let cfg: EneConfig = fig
            .extract()
            .expect("env-var override merges into defaults");

        unsafe {
            std::env::remove_var("ENE_TEST_provider__api_key");
        }

        let keys = extra_keys(&cfg);
        assert!(
            keys.contains(&"provider".to_string()),
            "expected lowercase 'provider' key, got {keys:?}"
        );
    }

    /// Regression for #40: pre-fix, `load_full_config_from` called
    /// `figment.extract().unwrap_or_else(|e| { ... EneConfig::default() })`
    /// which silently reset the entire config to defaults on any
    /// extract failure. After the fix, the function returns
    /// `EneConfigError::GenericConfigError` instead.
    #[test]
    fn malformed_settings_json_returns_error_not_default() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        // Not valid JSON for an EneConfig.
        std::fs::write(&path, "{ this is not valid json }").expect("write invalid JSON fixture");

        let result = load_full_config_from(tmp.path(), &path);
        assert!(
            result.is_err(),
            "expected Err on malformed settings.json, got Ok"
        );
    }

    /// Empty `settings.json` is still acceptable because Figment
    /// falls back to `Serialized::defaults`. Ensure the success path
    /// stays green after the new `?` propagation.
    #[test]
    fn empty_settings_json_extracts_defaults() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("write empty settings fixture");

        let result = load_full_config_from(tmp.path(), &path);
        assert!(result.is_ok(), "empty settings.json should extract ok");
    }

    /// Regression for #47 (bug 3): `set_nested` used to
    /// silently drop the write when the path crossed
    /// a non-object leaf (e.g. a user's settings.json
    /// has `"provider": "some string"` and the
    /// `set_section` path is `["provider", "api_key"]`).
    /// Now the write returns a typed error.
    #[test]
    fn set_nested_through_non_object_leaf_errors() {
        let mut extra: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        // Pre-populate a leaf where the path expects an
        // object.
        extra.insert(
            "provider".to_string(),
            serde_json::Value::String("some string".to_string()),
        );

        let result = set_nested(
            &mut extra,
            &["provider", "api_key"],
            serde_json::Value::String("sk-test".to_string()),
        );
        assert!(
            result.is_err(),
            "expected error on non-object leaf, got Ok with extra={extra:?}"
        );
        // The original leaf must not be silently clobbered.
        assert_eq!(
            extra.get("provider"),
            Some(&serde_json::Value::String("some string".to_string())),
            "non-object leaf should not be replaced with a fresh object"
        );
    }

    #[test]
    fn set_path_writes_dotted_json_value() {
        let mut config = EneConfig::default();
        config
            .set_path("ai.tasks.chat.model", "gpt-test")
            .expect("set_path");
        let value = config.get_path("ai.tasks.chat.model").expect("get_path");
        assert_eq!(value, serde_json::Value::String("gpt-test".to_string()));
    }
}
