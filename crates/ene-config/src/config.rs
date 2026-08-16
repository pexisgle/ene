use crate::error::EneConfigError;
use crate::user_persona::UserPersona;
use indexmap::IndexMap;
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Relative `$schema` pointer auto-filled into `settings.json` on save.
///
/// Matches the on-disk convention and the `schema/settings.schema.json` layout
/// produced by [`write_schemas`], so editors resolve completions without the
/// user hand-writing the key.
pub const DEFAULT_SETTINGS_SCHEMA: &str = "./schema/settings.schema.json";

/// Global singleton holding the active [`EneConfig`].
///
/// Uses `parking_lot::RwLock` which does not poison on panic, matching the
/// `ConfigStore` lock strategy.
pub static GLOBAL_CONFIG: std::sync::OnceLock<parking_lot::RwLock<EneConfig>> =
    std::sync::OnceLock::new();

pub fn update_global_config(config: EneConfig) {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        *lock.write() = config;
    } else {
        // If another thread raced us and set it first, that write already
        // landed, so a failed `set` here is a no-op we can safely discard.
        drop(GLOBAL_CONFIG.set(parking_lot::RwLock::new(config)));
    }
}

pub fn get_global_config() -> EneConfig {
    if let Some(lock) = GLOBAL_CONFIG.get() {
        return lock.read().clone();
    }
    EneConfig::default()
}

pub trait HasConfigKey {
    /// The string key of this configuration section under its parent.
    const KEY: &'static str;

    /// The target configuration file (Settings or Character).
    const TARGET: ConfigTarget;

    /// The full path from the root.
    fn path() -> &'static [&'static str];
}

pub fn get_global_section<T>() -> T
where
    T: serde::de::DeserializeOwned + Default + HasConfigKey,
{
    if let Some(lock) = GLOBAL_CONFIG.get() {
        return lock.read().get_section::<T>().unwrap_or_default();
    }
    T::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigTarget {
    /// Config belongs to settings.json
    Settings,
    /// Config belongs to `character_settings.json`
    Character,
}

pub struct SchemaEntry {
    pub schema: schemars::Schema,
    pub target: ConfigTarget,
    pub parent_key: Option<String>,
}

static SCHEMA_REGISTRY: std::sync::OnceLock<parking_lot::Mutex<HashMap<String, SchemaEntry>>> =
    std::sync::OnceLock::new();

/// Exposed so `ene-card`'s character-schema generator can merge
/// `ConfigTarget::Character` registrations without sharing the registry itself.
pub fn registered_schemas_for(target: ConfigTarget) -> Vec<(String, SchemaEntry)> {
    let Some(registry) = SCHEMA_REGISTRY.get() else {
        return Vec::new();
    };
    let reg = registry.lock();
    reg.iter()
        .filter(|(_, entry)| entry.target == target)
        .map(|(key, entry)| {
            (
                key.clone(),
                SchemaEntry {
                    schema: entry.schema.clone(),
                    target: entry.target,
                    parent_key: entry.parent_key.clone(),
                },
            )
        })
        .collect()
}

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

pub const DEFAULT_RUNTIME_RULES: &str =
    "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.";

fn runtime_rules_is_default(rules: &str) -> bool {
    rules.is_empty() || rules == DEFAULT_RUNTIME_RULES
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct EneConfig {
    /// Declared first so it always serialises at the top of `settings.json`
    /// (ahead of `version`), and skipped while empty so in-memory defaults
    /// carry no bogus path. [`save_full_config`] auto-fills it on save.
    #[serde(rename = "$schema", default, skip_serializing_if = "String::is_empty")]
    pub schema: String,
    pub version: u32,
    /// Character card name or path.
    pub character: String,
    pub user_name: String,
    /// Behavioural rules injected into every system prompt.
    #[serde(default, skip_serializing_if = "runtime_rules_is_default")]
    pub runtime_rules: String,

    /// When set, the `{{user_persona}}` CBS macro expands to this persona's fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_persona: Option<UserPersona>,

    #[serde(flatten)]
    #[schemars(skip)]
    /// An [`IndexMap`] so the user's hand-arranged section order is preserved
    /// across a save and newly added sections append at the end. Its
    /// `PartialEq` is order-insensitive, so the "skip if unchanged" guards keep
    /// their previous behaviour.
    ///
    /// figment 0.10's `Dict` is unconditionally a `BTreeMap`, so the extract
    /// hands this map back in alphabetical order; [`load_full_config_from`]
    /// re-sorts it into the file's original top-level key order immediately
    /// after loading so the user's order survives the load → mutate → save
    /// cycle.
    pub extra: IndexMap<String, serde_json::Value>,
}

impl Default for EneConfig {
    fn default() -> Self {
        Self {
            schema: String::new(),
            version: 1,
            character: "Alicia".to_string(),
            user_name: "User".to_string(),
            runtime_rules: DEFAULT_RUNTIME_RULES.to_string(),
            user_persona: None,
            extra: IndexMap::new(),
        }
    }
}

impl EneConfig {
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
        // Walk the path directly through the map,
        // descending into nested objects one level at a
        // time. The previous form rebuilt the entire
        // `extra` map into a JSON object on every call
        // (O(n) per read) and required cloning every value.
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

    /// Only the section's *declared* fields are written; unknown *immediate
    /// child* keys already present at the section path are preserved.
    /// The merge is one level deep: declared fields that are themselves objects
    /// (e.g. `plugins.list`, `ai.tasks`) are replaced wholesale, so unknown
    /// keys nested *beneath* them do not survive. This replaces the previous
    /// whole-subtree replacement, which silently wiped nested sibling sections
    /// such as `tools.rag` when writing `ToolRuntimeConfig`.
    ///
    /// Serialisation goes through [`section_to_value`] to avoid the f32→f64
    /// widening artefact that `serde_json::to_value` introduces.
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
        let val = section_to_value(section)?;
        let path = T::path();
        // Merge the declared fields over the existing subtree so unknown
        // immediate child keys survive, then skip the write when the merged
        // result is identical to what already sits at this path. This avoids
        // redundant map mutations and prevents unnecessary dirty-flag flips.
        let existing = read_at_path(&self.extra, path);
        let merged = merge_section(existing, &val);
        if existing.is_some_and(|current| current == &merged) {
            return Ok(());
        }
        set_nested(&mut self.extra, path, merged)?;
        Ok(())
    }

    /// `value` is parsed as JSON when possible; otherwise treated as a string.
    /// Used by CLI `/config set`.
    ///
    /// `$schema` is routed to the declared [`schema`](Self::schema) field
    /// rather than `extra`; writing it into `extra` would put a second
    /// `$schema` key on disk next to the declared field, and the resulting
    /// duplicate field would fail to reload.
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
        if path == ["$schema"] {
            self.schema = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            return Ok(());
        }
        set_nested(&mut self.extra, &path, value)
    }

    /// This is the generic counterpart of [`set_section`](Self::set_section)
    /// for callers that hold a section as an opaque JSON value (the runtime's
    /// unified settings-apply path diffs sections generically rather than
    /// knowing every typed section). The value is written verbatim — unlike
    /// [`set_section`](Self::set_section) there is no declared-field merge,
    /// so callers must merge themselves when unknown keys must survive.
    /// Writes are skipped when the value already equals what sits at the key.
    pub fn set_section_value(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), EneConfigError> {
        if key.is_empty() {
            return Err(EneConfigError::GenericConfigError(
                "empty section key".to_string(),
            ));
        }
        if self.extra.get(key).is_some_and(|current| current == &value) {
            return Ok(());
        }
        set_nested(&mut self.extra, &[key], value)
    }

    /// `None` when the section is absent (i.e. all defaults). Top-level
    /// declared fields (`character`, `user_name`, …) are *not* returned;
    /// use [`get_path`](Self::get_path) with the exact key for those.
    #[must_use]
    pub fn section_value(&self, key: &str) -> Option<serde_json::Value> {
        self.get_path(key)
    }

    /// Returns whether the key was present. Used by the unified settings-apply
    /// path when a section disappears from the proposed config (a deleted
    /// plugin entry or a cleared section), where writing `null` would leave a
    /// misleading literal behind.
    pub fn remove_section(&mut self, key: &str) -> bool {
        self.extra.shift_remove(key).is_some()
    }

    /// Walks the map directly instead of serialising the entire `extra`
    /// map into a JSON `Value` tree. `$schema` reads from the declared
    /// [`schema`](Self::schema) field, mirroring [`set_path`](Self::set_path).
    pub fn get_path(&self, dotted_path: &str) -> Option<serde_json::Value> {
        let keys: Vec<&str> = dotted_path.split('.').filter(|s| !s.is_empty()).collect();
        if keys.is_empty() {
            return None;
        }
        if keys == ["$schema"] {
            return Some(serde_json::Value::String(self.schema.clone()));
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

/// Serialises a typed config section into a [`serde_json::Value`] without the
/// f32→f64 widening artefact that `serde_json::to_value` introduces.
///
/// # Why not `to_value` directly?
///
/// `serde_json::to_value` routes f32 through `Number::from_f32`, which stores
/// `f as f64` internally. When the resulting `Value` tree is later written to
/// disk, ryu formats the *widened* f64, producing 17-digit noise such as
/// `0.6000000238418579` instead of `0.6`.
///
/// The string round-trip avoids this: `serde_json::to_string` calls ryu's
/// native f32 formatter (shortest representation that round-trips to the same
/// f32), and parsing the string back yields an f64 whose shortest decimal
/// representation is identical (e.g. `0.6f32` → `"0.6"` → `0.6f64` → `"0.6"`).
/// The f32 value is preserved because `0.6f64 as f32 == 0.6f32`.
pub fn section_to_value<T: serde::Serialize>(
    section: &T,
) -> Result<serde_json::Value, EneConfigError> {
    let json_str = serde_json::to_string(section).map_err(|e| {
        EneConfigError::GenericConfigError(format!("Failed to serialize section: {e}"))
    })?;
    serde_json::from_str(&json_str).map_err(|e| {
        EneConfigError::GenericConfigError(format!("Failed to parse serialized section: {e}"))
    })
}

/// Returns `None` when any key is absent or a non-object is encountered
/// before the final key.
pub fn read_at_path<'a>(
    extra: &'a IndexMap<String, serde_json::Value>,
    path: &[&str],
) -> Option<&'a serde_json::Value> {
    let mut current: Option<&serde_json::Value> = None;
    for (i, key) in path.iter().enumerate() {
        if i == 0 {
            current = extra.get(*key);
        } else {
            current = current
                .and_then(|v| v.as_object())
                .and_then(|o| o.get(*key));
        }
    }
    current
}

/// When both sides are JSON objects, the section's declared fields are layered
/// on top of the existing object so unknown sibling sub-keys survive; the
/// section struct only ever serialises its declared fields, so a shallow merge
/// is exactly "write declared fields, keep everything else". In every other
/// case (no existing value, or a non-object on either side) the incoming value
/// replaces the subtree outright.
///
/// # Caveat: `skip_serializing_if` fields cannot be deleted through merge
///
/// A field annotated with `skip_serializing_if` is *absent* from `incoming`
/// when it holds its skip value, so the merge keeps whatever stale value the
/// on-disk subtree already had — the field can never be cleared back to its
/// skipped state this way. No `define_config!` section struct uses
/// `skip_serializing_if` today, so this is a latent trap rather than a live
/// bug; adding one to a merged section would silently defeat deletion.
pub fn merge_section(
    existing: Option<&serde_json::Value>,
    incoming: &serde_json::Value,
) -> serde_json::Value {
    match (existing, incoming) {
        (Some(serde_json::Value::Object(base)), serde_json::Value::Object(overlay)) => {
            let mut merged = base.clone();
            for (key, value) in overlay {
                merged.insert(key.clone(), value.clone());
            }
            serde_json::Value::Object(merged)
        }
        _ => incoming.clone(),
    }
}

pub fn set_nested(
    extra: &mut IndexMap<String, serde_json::Value>,
    path: &[&str],
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    // Descend through the map, mutating the path in place. The previous form
    // rebuilt the entire `extra` map into a JSON object (O(n) on every write)
    // and silently dropped the write if `cur` ever landed on a non-object leaf.
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

/// Applies the user's in-session edits onto the raw on-disk JSON layer.
///
/// This is a three-way merge keyed on `base` — the layered config
/// (defaults → JSON → env) the loader produced — which is the common ancestor
/// of both `raw` (the JSON layer on disk) and `current` (the in-memory config
/// after the user's edits):
///
/// - a key whose value is unchanged between `base` and `current` keeps the raw
///   on-disk value (so env overrides and defaults never reach disk);
/// - a key the user changed (or added) takes the `current` value;
/// - a key present in `base` but absent from `current` was cleared by the user
///   (e.g. an `Option` field reset to `None`, which `skip_serializing_if`
///   omits from serialisation) and is removed from the output.
///
/// The env layer cancels out of the comparison because it is present in both
/// `base` and `current`, so env overrides stay transient. Unknown fields are
/// preserved, and the output is built in the raw file's key order (user-added
/// keys are appended after it), so a load→save round-trip does not reorder the
/// user's `settings.json`.
///
/// This assumes the `ENE_` environment is stable between load and save (the
/// normal case — the process environment does not change at runtime). If an
/// override is unset mid-process, the value it injected at load looks like a
/// user change relative to the rebuilt baseline and would be persisted.
fn three_way_merge(
    base: &serde_json::Value,
    raw: &serde_json::Value,
    current: &serde_json::Value,
) -> serde_json::Value {
    use serde_json::Value;

    match (base, raw, current) {
        (Value::Object(base_obj), Value::Object(raw_obj), Value::Object(cur_obj)) => {
            let mut out = serde_json::Map::new();
            // Iterate the raw on-disk object first so pre-existing keys keep
            // their file order in the saved document.
            for (key, raw_val) in raw_obj {
                match cur_obj.get(key) {
                    None => {
                        if !base_obj.contains_key(key) {
                            out.insert(key.clone(), raw_val.clone());
                        }
                    }
                    Some(cur_val) => match base_obj.get(key) {
                        Some(base_val) if base_val == cur_val => {
                            out.insert(key.clone(), raw_val.clone());
                        }
                        Some(base_val) => {
                            out.insert(key.clone(), three_way_merge(base_val, raw_val, cur_val));
                        }
                        // Not in base: the user added this key in-session.
                        None => {
                            out.insert(key.clone(), cur_val.clone());
                        }
                    },
                }
            }
            // Keys in `current` with no raw counterpart: persist only when the
            // user actually changed or added them (base == current means the
            // value came from defaults or the env layer and must not be baked).
            for (key, cur_val) in cur_obj {
                if !out.contains_key(key) && base_obj.get(key) != Some(cur_val) {
                    out.insert(key.clone(), cur_val.clone());
                }
            }
            Value::Object(out)
        }
        // Non-object (or shape-mismatched) values: the user's current value
        // wins when it differs from the base, otherwise the raw value stays.
        // (The `base == current` case is what a scalar that the user never
        // touched reaches; shape mismatches between the layers fall through
        // here too and resolve to `current`.)
        _ => {
            if base == current {
                raw.clone()
            } else {
                current.clone()
            }
        }
    }
}

/// Serialises only the JSON layer of `config` for persistence.
///
/// The in-memory [`EneConfig`] is the result of layering defaults → JSON file
/// → `ENE_` env vars, so serialising it directly would bake transient env
/// overrides (and every default) into `settings.json`. Instead this rebuilds
/// the same layered baseline the loader produced and three-way merges the
/// in-memory config onto the **raw** on-disk JSON, so only genuine user edits
/// are persisted. See [`three_way_merge`] for the merge semantics.
fn serialize_json_layer(config: &EneConfig, config_path: &Path) -> Result<String, EneConfigError> {
    // Run the same migration the loader runs, so the baseline and the raw
    // layer agree even when the file is behind the current version (on a
    // read-only filesystem the migration cannot persist, so reading the
    // raw file directly would compare a migrated baseline against an
    // unmigrated raw layer and rewrite the whole document).
    let raw_layer =
        serde_json::from_str::<serde_json::Value>(&migrate_settings_file(config_path)?)?;

    let baseline = extract_layered_config(config_path)?;
    let baseline_val = serde_json::to_value(&baseline)?;
    let mut current_val = serde_json::to_value(config)?;

    // A stray `$schema` left in the catch-all section must never win.
    // When the declared field is empty it serialises as absent, so drop any
    // `extra["$schema"]` from the current layer — the post-merge autofill
    // below supplies the canonical pointer. When the user set the declared
    // field, that key (which serialises at the top level) is kept verbatim.
    if config.schema.is_empty()
        && let Some(obj) = current_val.as_object_mut()
    {
        obj.remove("$schema");
    }

    let mut merged = three_way_merge(&baseline_val, &raw_layer, &current_val);

    // The persisted file always leads with the `$schema` pointer. The
    // declared field auto-fills when empty, and any stray `$schema` entry the
    // user (or an old save) left in the catch-all section is stripped so the
    // declared field wins and the key is never duplicated.
    if let Some(obj) = merged.as_object_mut() {
        let has_declared = obj.get("$schema").is_some();
        if !has_declared {
            obj.insert(
                "$schema".to_string(),
                serde_json::Value::String(DEFAULT_SETTINGS_SCHEMA.to_string()),
            );
        }
        // Re-insert at the front so `$schema` always leads.
        if let Some(schema_val) = obj.remove("$schema") {
            let mut reordered = serde_json::Map::new();
            reordered.insert("$schema".to_string(), schema_val);
            for (k, v) in obj.iter() {
                reordered.insert(k.clone(), v.clone());
            }
            *obj = reordered;
        }
    }

    Ok(serde_json::to_string_pretty(&merged)?)
}

pub fn generate_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<EneConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(registry) = SCHEMA_REGISTRY.get()
        && let Some(root_obj) = root_val.as_object_mut()
    {
        let reg = registry.lock();
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

/// Returns [`EneConfigError`] if the on-disk `settings.json` is malformed,
/// env-var parsing fails, or required fields cannot be deserialised.
pub fn load_config() -> Result<EneConfig, EneConfigError> {
    let config_path = crate::paths::config_file_path();
    load_config_from(&config_path)
}

pub fn load_config_from(config_path: &Path) -> Result<EneConfig, EneConfigError> {
    load_full_config_from(config_path)
}

pub fn load_full_config() -> Result<EneConfig, EneConfigError> {
    let config_path = crate::paths::config_file_path();
    load_full_config_from(&config_path)
}

/// Migration happens on the *raw JSON* — before deserialisation into
/// [`EneConfig`] — because a schema change may alter a field's type and make
/// the old file undecodable by the current struct (see the
/// [`crate::migration`] module docs). When the file's `version` is behind
/// [`CURRENT_CONFIG_VERSION`](crate::migration::CURRENT_CONFIG_VERSION) the
/// migrated document is persisted back to disk via [`atomic_write`] so the new
/// version survives the load; a file already at the current version is left
/// untouched.
///
/// A missing file yields `"{}"`, letting figment fall back to
/// `Serialized::defaults`. A file that exists but is not valid JSON is an
/// error, preserving the fail-loud behaviour.
fn migrate_settings_file(config_path: &Path) -> Result<String, EneConfigError> {
    let raw = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("{}".to_string()),
        Err(e) => return Err(EneConfigError::IoError(e)),
    };

    let doc: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        EneConfigError::GenericConfigError(format!(
            "failed to parse {}: {e}",
            config_path.display()
        ))
    })?;

    let from_version = crate::migration::document_version(&doc);
    let migrated = crate::migration::apply_migrations(doc)?;
    let migrated_text = serde_json::to_string_pretty(&migrated)?;

    // Persist only when the migration actually changed the document, so an
    // already-current file is never rewritten (and its mtime/permissions left
    // alone) on every load.
    if migrated_text != raw {
        // Pre-migration backup so a buggy rewrite is recoverable. Named after
        // the version being migrated *from*, so a later schema bump keeps its
        // own snapshot instead of being skipped because an older backup exists.
        // Written at most once per source version; failure is non-fatal (same
        // as a failed persist).
        let backup_path = {
            let mut os = config_path.as_os_str().to_owned();
            os.push(format!(".v{from_version}.bak"));
            PathBuf::from(os)
        };
        if !backup_path.exists() {
            if let Err(e) = std::fs::copy(config_path, &backup_path) {
                tracing::warn!(
                    component = "Config",
                    path = %config_path.display(),
                    backup = %backup_path.display(),
                    error = %e,
                    "could not back up settings.json before migration; continuing"
                );
            } else {
                tracing::info!(
                    component = "Config",
                    backup = %backup_path.display(),
                    "backed up settings.json before migration"
                );
            }
        }

        // A read-only filesystem (e.g. a packaged install) must not prevent
        // the app from starting: the migration already ran in memory, so the
        // load can proceed with the migrated document; the write is only a
        // convenience so the next load starts from the new version.
        if let Err(e) = atomic_write(config_path, &migrated_text) {
            tracing::warn!(
                component = "Config",
                path = %config_path.display(),
                error = %e,
                "could not persist migrated settings.json (read-only filesystem?); continuing with in-memory migration"
            );
        } else {
            tracing::info!(
                component = "Config",
                path = %config_path.display(),
                version = crate::migration::CURRENT_CONFIG_VERSION,
                "migrated settings.json to current config version"
            );
        }
    }

    Ok(migrated_text)
}

/// # Config-version migration
///
/// Before the figment pipeline runs, [`migrate_settings_file`] reads the raw
/// file and applies any registered
/// [config-version migrations](crate::migration), persisting the upgraded
/// document. The (possibly migrated) JSON is then fed to figment as a string
/// provider rather than re-reading the file, so the in-memory config and the
/// on-disk file always agree.
///
/// # Env-var case folding
///
/// The `ENE_` env provider applies `.map(|k| k.to_lowercase())` so that
/// `ENE_AI__TASKS__CHAT__MODEL` resolves to the `ai.tasks.chat.model` path
/// on [`EneConfig`] (lowercase). Without the case-folding, Figment stored
/// the path as `AI.tasks.chat.model` and the value was silently dropped
/// because `get_section::<AiConfig>()` looks up `T::path() = ["ai"]`
/// (lowercase).
pub fn load_full_config_from(config_path: &Path) -> Result<EneConfig, EneConfigError> {
    let config = extract_layered_config(config_path)?;
    update_global_config(config.clone());
    Ok(config)
}

/// Shared by [`load_full_config_from`] (the load path) and
/// [`serialize_json_layer`] (the save path). The save path needs the
/// exact same baseline the loader produced so it can isolate the user's
/// in-session mutations from the defaults and env layers.
fn extract_layered_config(config_path: &Path) -> Result<EneConfig, EneConfigError> {
    use figment::{
        Figment,
        providers::{Env, Format, Json, Serialized},
    };

    let settings_json = migrate_settings_file(config_path)?;

    let figment = Figment::from(Serialized::defaults(EneConfig::default()))
        .merge(Json::string(&settings_json))
        // `.map(...)` makes env vars case-insensitive against the
        // lowercase config keys, matching the documented
        // `ENE_AI__TASKS__CHAT__MODEL` examples.
        .merge(
            Env::prefixed("ENE_")
                .split("__")
                .map(|k| k.as_str().to_lowercase().into()),
        );

    let mut config: EneConfig = figment.extract().map_err(|e| {
        EneConfigError::GenericConfigError(format!("configuration extract failed: {e}"))
    })?;

    // figment's `Dict` is a `BTreeMap`, so `config.extra` comes back in
    // alphabetical order and the user's hand-arranged section order is lost.
    // Re-read the raw file once with serde_json (whose `preserve_order` feature
    // keeps insertion order) to recover the original top-level key order, then
    // re-sort `extra` into that order right here. Because the app lifecycle is
    // always load → mutate → save, fixing the order at load means the in-memory
    // `IndexMap` — and therefore every later save — keeps the user's order.
    // Newly added sections append at the end.
    restore_top_level_order(&mut config.extra, &read_top_level_order(config_path));

    update_global_config(config.clone());
    Ok(config)
}

/// Returns an empty `Vec` when the file is missing, unreadable, not valid
/// JSON, or not an object — ordering restoration then simply leaves `extra`
/// in the order figment produced, which is the pre-fix behaviour. This is
/// best-effort: it must never turn a successful config load into a failure.
fn read_top_level_order(config_path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    map.keys().cloned().collect()
}

/// Character schemas are written by the `ene-card` crate's
/// `write_character_schemas`.
///
/// Guarded by a process-wide [`std::sync::Once`] so the (idempotent but
/// wasteful) schema regeneration runs exactly once per process, even though
/// several startup entry points (CLI `init`, desktop `first_launch_setup`,
/// runtime `open_from_disk`/`open_with_config`) all call it. Each
/// schema file is written via [`atomic_write`] so a crash mid-write can
/// never leave a truncated schema behind.
pub fn write_schemas(assets_dir: &Path) {
    static WRITE_SCHEMAS_ONCE: std::sync::Once = std::sync::Once::new();
    WRITE_SCHEMAS_ONCE.call_once(|| write_schemas_inner(assets_dir));
}

fn write_schemas_inner(assets_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(assets_dir.join("schema")) {
        tracing::error!(component = "Config", error = %e, "Failed to create schema directory");
        return;
    }

    let schema_path = crate::paths::schema_file_path();
    if let Ok(schema_json) = generate_schema_json()
        && let Err(e) = atomic_write(&schema_path, &schema_json)
    {
        tracing::error!(component = "Config", path = %schema_path.display(), error = %e, "Failed to write settings schema");
    }
}

/// Atomically writes `contents` to `path` by first writing to a temporary
/// file in the same directory, then renaming over the target.
///
/// The rename is atomic on POSIX when source and destination reside on the
/// same filesystem, which is guaranteed by placing the temp file in the
/// target's parent directory. This prevents partial or corrupt config files
/// if the process crashes mid-write.
///
/// The temporary file name embeds the process id and a monotonic counter so
/// concurrent writers targeting the same path never collide on the temp
/// name. On Unix, an existing target's permission bits are copied onto the
/// temp file before the rename so a tightened mode (e.g. `0600` on a
/// `settings.json` holding `provider.api_key`) survives the rewrite.
///
/// # Durability
///
/// The file contents are `fsync`ed before the rename and the parent
/// directory afterwards, but both are best-effort: on filesystems where
/// `fsync` is a no-op (or fails) the rename is still atomic — a crash can
/// only ever leave either the old or the new file, never a partial one —
/// but the rename itself may not be durable across a power loss.
pub fn atomic_write(path: &Path, contents: &str) -> Result<(), EneConfigError> {
    use std::io::Write;

    let dir = path.parent().ok_or_else(|| {
        EneConfigError::GenericConfigError(format!("no parent directory for {}", path.display()))
    })?;
    std::fs::create_dir_all(dir).map_err(EneConfigError::IoError)?;

    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("config");
    let tmp_path = dir.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        tmp_counter()
    ));

    let mut file = std::fs::File::create(&tmp_path).map_err(EneConfigError::IoError)?;
    file.write_all(contents.as_bytes())
        .map_err(EneConfigError::IoError)?;

    preserve_permissions(path, &file);

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
    fsync_dir(dir);
    Ok(())
}

/// Monotonic counter so successive temp files in the same process get
/// distinct names even when written within the same millisecond.
fn tmp_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Copies the permission bits from an existing `src` file onto `dst`.
///
/// A missing source (first write) or any metadata error is ignored: the
/// temp file simply keeps the default mode. This is best-effort hardening,
/// not a correctness requirement of the atomic replace.
#[cfg(unix)]
fn preserve_permissions(src: &Path, dst: &std::fs::File) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Ok(meta) = std::fs::metadata(src) else {
        return;
    };
    let perms = std::fs::Permissions::from_mode(meta.mode() & 0o7777);
    if let Err(e) = dst.set_permissions(perms) {
        tracing::debug!(
            component = "Config",
            path = %src.display(),
            error = %e,
            "best-effort permission preservation failed (non-fatal)"
        );
    }
}

/// Non-Unix platforms: nothing to preserve beyond the default mode.
#[cfg(not(unix))]
fn preserve_permissions(_src: &Path, _dst: &std::fs::File) {}

/// Best-effort `fsync` of a directory so the preceding `rename` is durable.
///
/// Not all platforms/filesystems support syncing a directory; any error is
/// logged at debug level and ignored. The atomic-replace guarantee does not
/// depend on this — it only affects durability across a power loss.
fn fsync_dir(dir: &Path) {
    match std::fs::File::open(dir) {
        Ok(handle) => {
            if let Err(e) = handle.sync_all() {
                tracing::debug!(
                    component = "Config",
                    path = %dir.display(),
                    error = %e,
                    "best-effort directory fsync failed (non-fatal)"
                );
            }
        }
        Err(e) => {
            tracing::debug!(
                component = "Config",
                path = %dir.display(),
                error = %e,
                "could not open directory for fsync (non-fatal)"
            );
        }
    }
}

/// Keys listed in `order` come first (in that order); any key absent from
/// `order` — a section added after load — keeps its existing relative position
/// but sorts after the recorded ones. An empty `order` (e.g. a config built in
/// memory, or a load where the file order could not be recovered) leaves
/// `extra` untouched.
fn restore_top_level_order(extra: &mut IndexMap<String, serde_json::Value>, order: &[String]) {
    if order.is_empty() {
        return;
    }
    extra.sort_by(|a, _, b, _| {
        let rank = |key: &String| order.iter().position(|o| o == key);
        match (rank(a), rank(b)) {
            (Some(a_pos), Some(b_pos)) => a_pos.cmp(&b_pos),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

/// Only the JSON layer is persisted: `ENE_` env-var overrides and defaults are
/// excluded so a transient env override never becomes permanent. See
/// [`serialize_json_layer`] for the layer-reconstruction details.
///
/// `$schema` is auto-filled on the serialised copy when empty so the persisted
/// file always leads with the schema pointer.
///
/// # Concurrent edits
///
/// The three-way merge bases the comparison on the file as it was at *load*
/// time. If the file is modified externally between load and save, those
/// external edits are treated as if they were never made and are overwritten.
/// Concurrent writers must coordinate at a higher level (the desktop app
/// serialises saves on its own thread).
pub fn save_full_config(config: &EneConfig) -> Result<(), EneConfigError> {
    update_global_config(config.clone());
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(EneConfigError::IoError)?;
    }
    let json = serialize_json_layer(config, &config_path)?;
    atomic_write(&config_path, &json)?;
    Ok(())
}

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
    /// mutex serializes them. The `load_full_config_from` tests also take it:
    /// their `ENE_`-prefixed provider would otherwise pick up a concurrent
    /// test's `ENE_TEST_*` variable and grow `extra` with a stray key.
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

    /// Regression: the case-folding `.map(|k| k.to_lowercase())`
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

    /// Acquires the migration test lock so a load-path test cannot run while a
    /// [`crate::migration::tests::with_test_version`] test has a partially
    /// installed override (target version bumped, registry not yet populated).
    /// Without this, `load_full_config_from` — which now runs migrations — could
    /// observe that window and fail spuriously under parallel test threads.
    fn migration_guard() -> impl Drop {
        crate::migration::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Regression: pre-fix, `load_full_config_from` called
    /// `figment.extract().unwrap_or_else(|e| { ... EneConfig::default() })`
    /// which silently reset the entire config to defaults on any
    /// extract failure. After the fix, the function returns
    /// `EneConfigError::GenericConfigError` instead.
    #[test]
    fn malformed_settings_json_returns_error_not_default() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        // Not valid JSON for an EneConfig.
        std::fs::write(&path, "{ this is not valid json }").expect("write invalid JSON fixture");

        let result = load_full_config_from(&path);
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
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("write empty settings fixture");

        let result = load_full_config_from(&path);
        let config = result.expect("empty settings.json should extract ok");
        assert_eq!(config.version, crate::migration::CURRENT_CONFIG_VERSION);
        assert_eq!(config.user_name, EneConfig::default().user_name);
        assert!(runtime_rules_is_default(&config.runtime_rules));
    }

    #[test]
    fn load_migrates_old_version_and_persists() {
        crate::migration::tests::with_test_version(2, || {
            // v1 -> v2: rename `name` to `user_name`.
            crate::migration::register_migration(1, |doc| {
                if let Some(obj) = doc.as_object_mut()
                    && let Some(name) = obj.remove("name")
                {
                    obj.insert("user_name".to_string(), name);
                }
                Ok(())
            })
            .expect("registration below current version succeeds");

            let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
            let path = tmp.path().join("settings.json");
            std::fs::write(&path, r#"{"version": 1, "name": "Hoshino"}"#)
                .expect("write old-version settings fixture");

            let config = load_full_config_from(&path).expect("old-version config loads");
            assert_eq!(config.version, 2, "loaded config carries the new version");
            assert_eq!(config.user_name, "Hoshino", "migrated field is visible");

            let on_disk: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read back"))
                    .expect("persisted JSON is valid");
            assert_eq!(
                on_disk.get("version"),
                Some(&serde_json::json!(2)),
                "migrated version must be persisted to disk"
            );
            assert_eq!(
                on_disk.get("user_name"),
                Some(&serde_json::json!("Hoshino"))
            );
            assert!(
                on_disk.get("name").is_none(),
                "old field must be rewritten away"
            );

            let backup = path.with_file_name("settings.json.v1.bak");
            let backup_raw = std::fs::read_to_string(&backup).expect("pre-migration backup exists");
            assert!(
                backup_raw.contains(r#""name": "Hoshino""#),
                "backup must preserve the pre-migration document"
            );
            assert!(
                !backup_raw.contains(r#""user_name""#),
                "backup must not contain post-migration fields"
            );
        });
    }

    /// End-to-end for the real v1→v2 step: a version-1 `settings.json` holding
    /// the relocated keys is migrated to version 2 on load, persisted, and the
    /// plugin-owned settings land under `plugins.list.*`.
    #[test]
    fn load_migrates_v1_relocated_settings_to_v2() {
        crate::migration::tests::with_test_version(2, || {
            crate::migration::register_migration(1, crate::migration::migrate_v1_to_v2)
                .expect("registration below current version succeeds");

            let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
            let path = tmp.path().join("settings.json");
            let v1 = r#"{
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "mmproj_url": "https://cdn.example/mmproj.gguf",
                            "acceleration": "auto"
                        }
                    },
                    "ort_dylib_path": "/opt/onnx/libonnxruntime.so",
                    "tts": { "voices_path": "/data/voices.bin" }
                }
            }"#;
            std::fs::write(&path, v1).expect("write old-version settings fixture");

            let config = load_full_config_from(&path).expect("old-version config loads");
            assert_eq!(config.version, 2, "loaded config carries the new version");
            assert_eq!(
                config.get_path("plugins.list.llama-cpp.config.mmproj_url"),
                Some(serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
            assert_eq!(
                config.get_path("plugins.list.onnx.config.ort_dylib_path"),
                Some(serde_json::json!("/opt/onnx/libonnxruntime.so"))
            );

            let on_disk: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read back"))
                    .expect("persisted JSON is valid");
            assert_eq!(
                on_disk.get("version"),
                Some(&serde_json::json!(2)),
                "migrated version must be persisted to disk"
            );
            assert!(
                on_disk.pointer("/ai/ort_dylib_path").is_none(),
                "old ai.ort_dylib_path must be gone from disk"
            );
        });
    }

    #[test]
    fn load_leaves_current_version_untouched() {
        crate::migration::tests::with_test_version(1, || {
            let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
            let path = tmp.path().join("settings.json");
            // Written pretty-printed, matching what `atomic_write` produces, so
            // a no-op migration yields byte-identical output.
            let original = "{\n  \"version\": 1,\n  \"character\": \"Alicia\"\n}";
            std::fs::write(&path, original).expect("write current-version settings fixture");

            let config = load_full_config_from(&path).expect("current-version config loads");
            assert_eq!(config.version, 1);
            assert_eq!(config.character, "Alicia");

            let after = std::fs::read_to_string(&path).expect("read back");
            assert_eq!(
                after, original,
                "a current-version file must not be rewritten on load"
            );
        });
    }

    /// A `settings.json` newer than the build supports is rejected with
    /// [`EneConfigError::ConfigVersionTooNew`] and left untouched, so a newer
    /// build can still read it after a downgrade.
    #[test]
    fn load_rejects_newer_version_without_touching_file() {
        crate::migration::tests::with_test_version(1, || {
            let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
            let path = tmp.path().join("settings.json");
            let original = r#"{"version": 99, "character": "Alicia"}"#;
            std::fs::write(&path, original).expect("write newer-version settings fixture");

            let err = load_full_config_from(&path).expect_err("newer-version config must error");
            assert!(
                matches!(
                    err,
                    EneConfigError::ConfigVersionTooNew {
                        found: 99,
                        supported: 1,
                    }
                ),
                "expected ConfigVersionTooNew, got {err:?}"
            );

            let after = std::fs::read_to_string(&path).expect("read back");
            assert_eq!(
                after, original,
                "a too-new file must not be modified by a failed load"
            );
        });
    }

    /// Regression (bug 3): `set_nested` used to
    /// silently drop the write when the path crossed
    /// a non-object leaf (e.g. a user's settings.json
    /// has `"provider": "some string"` and the
    /// `set_section` path is `["provider", "api_key"]`).
    /// Now the write returns a typed error.
    #[test]
    fn set_nested_through_non_object_leaf_errors() {
        let mut extra: IndexMap<String, serde_json::Value> = IndexMap::new();
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

    #[test]
    fn three_way_merge_keeps_raw_for_unchanged_and_drops_cleared() {
        use serde_json::json;
        // base = layered load (defaults + JSON + env); `env_key` is an env
        // override present in base and current, so it must NOT reach the output.
        let base = json!({
            "unchanged": 1,
            "changed": "old",
            "env_key": "from-env",
            "cleared": "was-set",
        });
        let raw = json!({
            "unchanged": 1,
            "changed": "old",
            "cleared": "was-set",
            "unknown": "preserve-me",
        });
        let current = json!({
            "unchanged": 1,
            "changed": "new",
            "env_key": "from-env",
            "added": true,
        });
        let merged = three_way_merge(&base, &raw, &current);
        assert_eq!(
            merged,
            json!({
                "unchanged": 1,
                "changed": "new",
                "added": true,
                "unknown": "preserve-me",
            }),
            "env_key must not persist, cleared must be dropped, unknown preserved"
        );
    }

    #[test]
    fn three_way_merge_recurses_into_nested_objects() {
        use serde_json::json;
        let base = json!({"section": {"keep": 1, "drop": 2, "edit": "a"}});
        let raw = json!({"section": {"keep": 1, "drop": 2, "edit": "a"}});
        let current = json!({"section": {"keep": 1, "edit": "b"}});
        let merged = three_way_merge(&base, &raw, &current);
        assert_eq!(
            merged,
            json!({"section": {"keep": 1, "edit": "b"}}),
            "nested cleared key must be dropped, edit applied"
        );
    }

    /// Regression: an `ENE_` env-var override applies at runtime but
    /// must NOT be baked into `settings.json` on save.
    #[test]
    fn env_override_not_persisted_on_save() {
        let _guard = migration_guard();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("seed empty settings");

        // SAFETY: serialized by ENV_LOCK; no other threads touch this env var.
        unsafe {
            std::env::set_var("ENE_AI__TASKS__CHAT__MODEL", "gpt-env-override");
        }

        let config = extract_layered_config(&path).expect("load");
        assert_eq!(
            config.get_path("ai.tasks.chat.model"),
            Some(serde_json::Value::String("gpt-env-override".to_string())),
            "env override must apply at runtime"
        );

        let json = serialize_json_layer(&config, &path).expect("serialize");
        unsafe {
            std::env::remove_var("ENE_AI__TASKS__CHAT__MODEL");
        }

        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(
            saved.get("ai").is_none(),
            "env override must not be persisted, got {json}"
        );
    }

    /// Regression: a genuine user change persists (layered onto the
    /// raw JSON) while a concurrent env override stays transient.
    #[test]
    fn genuine_change_persists_but_env_override_does_not() {
        let _guard = migration_guard();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"user_name":"DiskName"}"#).expect("seed settings");

        // SAFETY: serialized by ENV_LOCK; no other threads touch this env var.
        unsafe {
            std::env::set_var("ENE_AI__TASKS__CHAT__MODEL", "gpt-env");
        }

        let mut config = extract_layered_config(&path).expect("load");
        config.user_name = "ChangedByUser".to_string();

        let json = serialize_json_layer(&config, &path).expect("serialize");
        unsafe {
            std::env::remove_var("ENE_AI__TASKS__CHAT__MODEL");
        }

        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            saved.get("user_name"),
            Some(&serde_json::Value::String("ChangedByUser".to_string())),
            "genuine user change must persist, got {json}"
        );
        assert!(
            saved.get("ai").is_none(),
            "env override must not be persisted, got {json}"
        );
    }

    #[test]
    fn save_preserves_raw_key_order() {
        let _guard = migration_guard();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        // Deliberately out of alphabetical order: `zeta` before `alpha`.
        std::fs::write(&path, r#"{"zeta":"first","alpha":"second"}"#).expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        config.user_name = "Edited".to_string();

        let json = serialize_json_layer(&config, &path).expect("serialize");
        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let keys: Vec<&String> = saved
            .as_object()
            .expect("saved doc is an object")
            .keys()
            .collect();
        let zeta_pos = keys
            .iter()
            .position(|k| *k == "zeta")
            .expect("zeta present");
        let alpha_pos = keys
            .iter()
            .position(|k| *k == "alpha")
            .expect("alpha present");
        assert!(
            zeta_pos < alpha_pos,
            "raw key order must be preserved, got keys {keys:?}"
        );
    }

    /// Regression: saving an untouched config must not force default
    /// values into `settings.json`; the raw JSON layer is written back as-is.
    ///
    /// The one exception is the `version` field, which the config-version
    /// migration mechanism stamps explicitly on every document; a file
    /// without it is treated as version 1.
    #[test]
    fn defaults_not_forced_to_disk_on_save() {
        let _guard = migration_guard();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"user_name":"OnlyThis"}"#).expect("seed settings");

        let config = extract_layered_config(&path).expect("load");
        let json = serialize_json_layer(&config, &path).expect("serialize");

        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let keys: Vec<&str> = saved
            .as_object()
            .expect("saved config is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["$schema", "user_name", "version"],
            "only the on-disk key, the auto-filled $schema, and the version stamp should remain, got {json}"
        );
    }

    /// Regression: clearing an optional field (here `user_persona`,
    /// which `skip_serializing_if` omits when `None`) must be persisted — the
    /// stale on-disk value must not survive a save/reload cycle.
    #[test]
    fn cleared_optional_field_is_removed_from_disk() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"user_persona":{"name":"Alice"}}"#).expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        assert!(
            config.user_persona.is_some(),
            "persona should load from disk"
        );
        config.user_persona = None;

        let json = serialize_json_layer(&config, &path).expect("serialize");
        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(
            saved.get("user_persona").is_none(),
            "cleared optional field must be removed from disk, got {json}"
        );
        let mut config = EneConfig::default();
        config
            .set_path("ai.tasks.chat.model", "gpt-test")
            .expect("set_path");
        let value = config.get_path("ai.tasks.chat.model").expect("get_path");
        assert_eq!(value, serde_json::Value::String("gpt-test".to_string()));
    }

    /// A test-only settings section used to exercise `set_section` without
    /// pulling in another workspace crate (whose `define_config!` impl would
    /// be for a different copy of the `HasConfigKey` trait).
    ///
    /// Modelled on the real `ToolRuntimeConfig` (`tools`, owned by
    /// `ene-runtime`), which sits at the same path as the nested
    /// `ToolRagConfig` (`tools.rag`, owned by `ene-tool-rag`). Writing
    /// `tools` must not wipe the sibling `tools.rag` subtree — the exact
    /// regression the merge change fixes.
    #[derive(serde::Serialize, serde::Deserialize, Default)]
    struct TestSection {
        enabled: bool,
    }

    impl HasConfigKey for TestSection {
        const KEY: &'static str = "tools";
        const TARGET: ConfigTarget = ConfigTarget::Settings;
        fn path() -> &'static [&'static str] {
            &["tools"]
        }
    }

    /// Regression: writing a section must merge its declared fields
    /// into the existing subtree rather than replacing it, so an unknown
    /// sibling sub-key (here `tools.rag`, which `ToolRuntimeConfig` does not
    /// declare) survives the write.
    #[test]
    fn set_section_preserves_unknown_subkeys() {
        let mut config = EneConfig::default();
        config
            .set_path("tools.rag.enabled", "true")
            .expect("seed unknown sibling sub-key");
        config
            .set_path("tools.enabled", "false")
            .expect("seed declared field");

        config
            .set_section(&TestSection { enabled: true })
            .expect("set_section succeeds");

        assert_eq!(
            config.get_path("tools.enabled"),
            Some(serde_json::Value::Bool(true)),
            "declared field must be updated"
        );
        assert_eq!(
            config.get_path("tools.rag.enabled"),
            Some(serde_json::Value::Bool(true)),
            "unknown sibling sub-key must survive the section write"
        );
    }

    /// Host-opaque `plugins.list.<name>.config` / `.profiles` blobs are stored
    /// and restored verbatim through a load → save → load round-trip, so the
    /// host never drops plugin-owned settings (including keys it does not
    /// understand) when persisting.
    #[test]
    fn plugins_list_config_and_profiles_round_trip_verbatim() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "enable": true,
                            "config": {
                                "mmproj_url": "https://cdn.example/mmproj.gguf",
                                "future_field": {"nested": [1, 2, 3]}
                            },
                            "profiles": {
                                "default": {"voices_path": "/data/voices.bin"}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("seed settings");

        let config = extract_layered_config(&path).expect("load");
        // A genuine mutation elsewhere must still trigger a save and the
        // three-way merge must preserve the plugin blobs untouched.
        let mut config = config;
        config.user_name = "ChangedByUser".to_string();

        let json = serialize_json_layer(&config, &path).expect("serialize");
        let saved: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            saved.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
            Some(&serde_json::json!("https://cdn.example/mmproj.gguf")),
            "nested config key must survive the save"
        );
        assert_eq!(
            saved.pointer("/plugins/list/llama-cpp/config/future_field"),
            Some(&serde_json::json!({"nested": [1, 2, 3]})),
            "unknown nested config keys must survive the save"
        );
        assert_eq!(
            saved.pointer("/plugins/list/llama-cpp/profiles/default/voices_path"),
            Some(&serde_json::json!("/data/voices.bin")),
            "profiles must survive the save"
        );

        let reloaded = extract_layered_config(&path).expect("reload");
        assert_eq!(
            reloaded.get_path("plugins.list.llama-cpp.config.future_field"),
            Some(serde_json::json!({"nested": [1, 2, 3]}))
        );
        assert_eq!(
            reloaded.get_path("plugins.list.llama-cpp.profiles.default.voices_path"),
            Some(serde_json::json!("/data/voices.bin"))
        );
    }

    /// The `ENE_PLUGINS__LIST__<NAME>__CONFIG__<KEY>` env override path
    /// (single plugin-config key) must keep resolving into the nested
    /// `plugins.list.<name>.config` blob.
    #[test]
    fn plugins_list_config_env_override_resolves_nested_key() {
        let _guard = migration_guard();
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"plugins": {"list": {"anthropic": {"enable": true}}}}"#,
        )
        .expect("seed settings");

        // SAFETY: serialized by ENV_LOCK; no other threads touch this env var.
        unsafe {
            std::env::set_var(
                "ENE_PLUGINS__LIST__ANTHROPIC__CONFIG__API_KEY",
                "sk-env-override",
            );
        }
        let config = extract_layered_config(&path).expect("load");
        unsafe {
            std::env::remove_var("ENE_PLUGINS__LIST__ANTHROPIC__CONFIG__API_KEY");
        }

        assert_eq!(
            config.get_path("plugins.list.anthropic.config.api_key"),
            Some(serde_json::json!("sk-env-override")),
            "env override must land inside the nested config blob"
        );
    }

    /// Regression: re-writing an identical section must be a no-op so
    /// the "skip if unchanged" guard still holds after the merge change.
    #[test]
    fn set_section_identical_write_is_noop() {
        let mut config = EneConfig::default();
        config
            .set_section(&TestSection { enabled: true })
            .expect("first write");
        let before = config.extra.clone();
        config
            .set_section(&TestSection { enabled: true })
            .expect("second write");
        assert_eq!(before, config.extra, "identical write must not mutate");
    }

    /// Regression: `$schema` is the first declared field, so it must
    /// serialise ahead of `version` (which is second).
    #[test]
    fn schema_is_first_and_version_second() {
        let config = EneConfig {
            schema: DEFAULT_SETTINGS_SCHEMA.to_string(),
            ..EneConfig::default()
        };
        let value = serde_json::to_value(&config).expect("config serialises");
        let keys: Vec<&str> = value
            .as_object()
            .expect("config is an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys.first().copied(),
            Some("$schema"),
            "$schema must be the first key, got {keys:?}"
        );
        assert_eq!(
            keys.get(1).copied(),
            Some("version"),
            "version must be the second key, got {keys:?}"
        );
    }

    /// Regression: an empty `$schema` is auto-filled on save so users
    /// never hand-write it, and the caller's config is left untouched.
    #[test]
    fn save_autofills_schema_without_mutating_caller() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        config.schema.clear();
        assert!(config.schema.is_empty(), "default schema starts empty");

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            parsed.get("$schema").and_then(serde_json::Value::as_str),
            Some(DEFAULT_SETTINGS_SCHEMA),
            "saved JSON must carry the auto-filled $schema"
        );
        assert!(
            config.schema.is_empty(),
            "the caller's config must not be mutated by save"
        );
    }

    /// Regression: a non-empty `$schema` provided by the user is
    /// preserved verbatim on save (auto-fill only applies when empty).
    #[test]
    fn save_preserves_user_schema() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        config.schema = "./custom.schema.json".to_string();
        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            parsed.get("$schema").and_then(serde_json::Value::as_str),
            Some("./custom.schema.json")
        );
    }

    /// Regression: the user's hand-arranged top-level section order is
    /// preserved across a save (`IndexMap`, not alphabetical `BTreeMap`).
    #[test]
    fn section_order_preserved_on_save() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"store":{},"ai":{},"mind":{},"desktop":{}}"#)
            .expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        for section in ["store", "ai", "mind", "desktop"] {
            config.extra.insert(
                section.to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let keys: Vec<&str> = parsed
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let section_keys: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| matches!(*k, "store" | "ai" | "mind" | "desktop"))
            .collect();
        assert_eq!(
            section_keys,
            vec!["store", "ai", "mind", "desktop"],
            "section order must survive the save, got {section_keys:?}"
        );
    }

    /// Writes a `settings.json` fixture with a deliberately non-alphabetical
    /// top-level section order, for the load-path ordering regressions.
    fn write_ordered_settings_fixture(path: &Path) {
        let json = r#"{
  "version": 1,
  "store": { "enabled": true },
  "ai": { "tasks": {} },
  "mind": { "emotion": {} },
  "desktop": { "language": "en" }
}"#;
        std::fs::write(path, json).expect("write ordered settings fixture");
    }

    /// Regression: figment 0.10's `Dict` is a `BTreeMap`, so the
    /// extract hands `extra` back in alphabetical order. Going through the
    /// *real* load path (`load_full_config_from`, not in-memory construction),
    /// the load must re-sort `extra` into the file's original section order so
    /// the subsequent save keeps it.
    #[test]
    fn load_then_save_restores_file_section_order() {
        let _guard = migration_guard();
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        write_ordered_settings_fixture(&path);

        let config = load_full_config_from(&path).expect("settings load");

        let loaded: Vec<String> = config.extra.keys().cloned().collect();
        assert_eq!(
            loaded,
            vec!["store", "ai", "mind", "desktop"],
            "load must restore the file's section order, got {loaded:?}"
        );

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let section_keys: Vec<&str> = parsed
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .filter(|k| matches!(*k, "store" | "ai" | "mind" | "desktop"))
            .collect();
        assert_eq!(
            section_keys,
            vec!["store", "ai", "mind", "desktop"],
            "saved file must keep the user's section order, got {section_keys:?}"
        );
    }

    /// Regression: a section added after load (not present in the
    /// recorded order) must append after the file's original sections rather
    /// than being sorted into the middle of them.
    #[test]
    fn load_then_save_appends_new_section_after_recorded_order() {
        let _guard = migration_guard();
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        write_ordered_settings_fixture(&path);

        let mut config = load_full_config_from(&path).expect("settings load");
        config
            .set_path("plugins.enabled", "true")
            .expect("add a new section after load");

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let section_keys: Vec<&str> = parsed
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .filter(|k| matches!(*k, "store" | "ai" | "mind" | "desktop" | "plugins"))
            .collect();
        assert_eq!(
            section_keys,
            vec!["store", "ai", "mind", "desktop", "plugins"],
            "new section must append after the recorded order, got {section_keys:?}"
        );
    }

    /// Regression: setting `$schema` via `set_path` (reachable from
    /// CLI `/config set $schema …`) must route to the declared field, not
    /// `extra`. Otherwise two `$schema` keys land on disk and the reload fails
    /// with a "duplicate field" error — fatal since the silent fallback to
    /// defaults was removed.
    #[test]
    fn set_schema_via_set_path_round_trips() {
        let _guard = migration_guard();
        let _env_guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        write_ordered_settings_fixture(&path);

        let mut config = load_full_config_from(&path).expect("settings load");
        config
            .set_path("$schema", "./custom.schema.json")
            .expect("set $schema via set_path");

        assert_eq!(config.schema, "./custom.schema.json");
        assert!(
            !config.extra.contains_key("$schema"),
            "$schema must not leak into extra"
        );
        assert_eq!(
            config.get_path("$schema"),
            Some(serde_json::Value::String(
                "./custom.schema.json".to_string()
            )),
            "get_path must read the declared field"
        );

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        std::fs::write(&path, json).expect("persist settings");
        let reloaded = load_full_config_from(&path).expect("reload must not hit duplicate $schema");
        assert_eq!(reloaded.schema, "./custom.schema.json");
    }

    /// Defensive: even if a stray `extra["$schema"]` is present in memory,
    /// `serialize_json_layer` must drop it so the declared field is never
    /// duplicated on disk.
    #[test]
    fn serialize_for_save_strips_stray_schema_from_extra() {
        let _guard = migration_guard();
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("seed settings");

        let mut config = extract_layered_config(&path).expect("load");
        config.schema.clear();
        config.extra.insert(
            "$schema".to_string(),
            serde_json::Value::String("./stray.schema.json".to_string()),
        );

        let json = serialize_json_layer(&config, &path).expect("serialise for save");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let obj = parsed.as_object().expect("object");

        let schema_count = obj.keys().filter(|k| k.as_str() == "$schema").count();
        assert_eq!(schema_count, 1, "exactly one $schema key on disk");
        assert_eq!(
            obj.get("$schema").and_then(serde_json::Value::as_str),
            Some(DEFAULT_SETTINGS_SCHEMA),
            "the declared (auto-filled) field wins over the stray extra entry"
        );
    }

    #[test]
    fn atomic_write_produces_final_contents_and_no_tmp_leftover() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let target = tmp.path().join("settings.json");

        atomic_write(&target, "{\"hello\":\"world\"}").expect("atomic_write succeeds");

        let contents = std::fs::read_to_string(&target).expect("target is readable");
        assert_eq!(contents, "{\"hello\":\"world\"}");

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("dir readable")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp file should be renamed away, found leftovers: {leftovers:?}"
        );
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let target = tmp.path().join("settings.json");

        atomic_write(&target, "first-contents-that-is-quite-long")
            .expect("first atomic_write succeeds");
        atomic_write(&target, "short").expect("second atomic_write succeeds");

        let contents = std::fs::read_to_string(&target).expect("target is readable");
        assert_eq!(contents, "short");
    }

    /// On Unix, rewriting an existing file whose mode was tightened (e.g.
    /// `0600` for a `settings.json` holding `provider.api_key`) must not
    /// widen it back to the default `0644`.
    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_permissions() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let target = tmp.path().join("settings.json");

        std::fs::write(&target, "{}").expect("seed file");
        let mut perms = std::fs::metadata(&target).expect("metadata").permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&target, perms).expect("tighten mode");

        atomic_write(&target, "{\"k\":\"v\"}").expect("atomic_write succeeds");

        let mode = std::fs::metadata(&target).expect("metadata").mode() & 0o7777;
        assert_eq!(
            mode, 0o600,
            "tightened permissions must survive the rewrite"
        );
    }

    /// A struct with f32 fields, mirroring the shape of real config sections
    /// (e.g. `MindMemoryConfig`, `CharacterConfig`) that flow through
    /// `set_section` → `extra` map → `to_string_pretty`.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
    struct FloatSection {
        weight: f32,
        threshold: f32,
        scale: f32,
    }

    /// `section_to_value` must not introduce the 17-digit f32→f64 widening
    /// artefact. `0.3f32` should appear as `0.3` in the JSON output, not
    /// `0.30000001192092896`.
    #[test]
    fn section_to_value_f32_shortest_representation() {
        let section = FloatSection {
            weight: 0.3,
            threshold: 0.6,
            scale: 1.0,
        };
        let value = section_to_value(&section).expect("serialize");
        let json = serde_json::to_string(&value).expect("to_string");

        assert!(json.contains("0.3"), "expected 0.3 in output, got: {json}");
        assert!(
            !json.contains("0.30000001192092896"),
            "17-digit widening artefact found in: {json}"
        );
        assert!(json.contains("0.6"), "expected 0.6 in output, got: {json}");
        assert!(
            !json.contains("0.6000000238418579"),
            "17-digit widening artefact found in: {json}"
        );
    }

    #[test]
    fn section_to_value_f32_round_trips_exactly() {
        let section = FloatSection {
            weight: 0.3,
            threshold: 0.6,
            scale: 2.5,
        };
        let value = section_to_value(&section).expect("serialize");
        let json = serde_json::to_string_pretty(&value).expect("to_string_pretty");
        let recovered: FloatSection = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(recovered, section, "f32 values must survive the round-trip");
    }

    #[test]
    fn set_section_writes_clean_floats_into_extra() {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
        struct TestSection {
            strength: f32,
        }
        impl HasConfigKey for TestSection {
            const KEY: &'static str = "test_float";
            const TARGET: ConfigTarget = ConfigTarget::Settings;
            fn path() -> &'static [&'static str] {
                &["test_float"]
            }
        }

        let mut config = EneConfig::default();
        let section = TestSection { strength: 0.3 };
        config.set_section(&section).expect("set_section");

        let json = serde_json::to_string_pretty(&config).expect("serialize config");
        assert!(
            !json.contains("0.30000001192092896"),
            "17-digit artefact in full config output: {json}"
        );
        assert!(json.contains("0.3"), "expected clean 0.3 in: {json}");
    }
}
