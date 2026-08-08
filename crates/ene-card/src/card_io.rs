use ene_config::config::atomic_write;
use ene_config::paths::{character_card_schema_file_path, character_schema_file_path};
use ene_config::{ConfigTarget, EneConfigError};
use std::path::Path;

use crate::CharacterCardV3;
use crate::character_config::CharacterConfig;

/// Generates the JSON representation of the JSON Schema for `character_settings.json`
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<CharacterConfig>();
    let mut root_val = serde_json::to_value(&root_schema)?;

    if let Some(root_obj) = root_val.as_object_mut() {
        // 1. Copy definitions
        for (_, entry) in ene_config::config::registered_schemas_for(ConfigTarget::Character) {
            let entry_val = serde_json::to_value(&entry.schema)?;
            let def_key = if root_obj.contains_key("$defs") {
                "$defs"
            } else {
                "definitions"
            };
            if let Some(definitions) = entry_val
                .get("$defs")
                .or_else(|| entry_val.get("definitions"))
                .and_then(serde_json::Value::as_object)
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
        for (key, entry) in ene_config::config::registered_schemas_for(ConfigTarget::Character) {
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
    let root_schema = schema_gen.into_root_schema_for::<CharacterCardV3>();
    serde_json::to_string_pretty(&root_schema)
}

/// Serializes `card` and atomically writes it to `path`.
///
/// The counterpart of [`crate::load_character_card`]. The write goes through an
/// atomic temp-file-and-rename operation so a crash mid-write can never leave
/// a truncated card behind; host apps must not use a plain `fs::write` here.
pub fn save_character_card(path: &Path, card: &CharacterCardV3) -> Result<(), EneConfigError> {
    let json = serde_json::to_string_pretty(card).map_err(EneConfigError::SerializeError)?;
    atomic_write(path, &json)
}

/// Auto-generates and writes out the character and character-card schemas under
/// the assets schema directory.
///
/// Guarded by a process-wide [`std::sync::Once`] so the (idempotent but
/// wasteful) schema regeneration runs exactly once per process, even though
/// several startup entry points (CLI `init`, desktop `first_launch_setup`,
/// runtime `open_from_disk`/`open_with_config`) all call it. Each
/// schema file is written via [`atomic_write`] so a crash mid-write can
/// never leave a truncated schema behind.
pub fn write_character_schemas(assets_dir: &Path) {
    static WRITE_SCHEMAS_ONCE: std::sync::Once = std::sync::Once::new();
    WRITE_SCHEMAS_ONCE.call_once(|| {
        if let Err(e) = std::fs::create_dir_all(assets_dir.join("schema")) {
            tracing::error!(component = "Card", error = %e, "Failed to create schema directory");
            return;
        }

        let char_schema_path = character_schema_file_path();
        if let Ok(char_schema_json) = generate_character_schema_json()
            && let Err(e) = atomic_write(&char_schema_path, &char_schema_json)
        {
            tracing::error!(component = "Card", path = %char_schema_path.display(), error = %e, "Failed to write character schema");
        }

        let char_card_schema_path = character_card_schema_file_path();
        if let Ok(char_card_schema_json) = generate_character_card_schema_json()
            && let Err(e) = atomic_write(&char_card_schema_path, &char_card_schema_json)
        {
            tracing::error!(component = "Card", path = %char_card_schema_path.display(), error = %e, "Failed to write character card schema");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `save_character_card` must write a valid, re-loadable card and leave no
    /// temp-file residue behind — the atomic-write contract for card saves.
    #[test]
    fn save_character_card_writes_valid_card_without_temp_residue() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let mut card = crate::CharacterCardV3::default();
        card.data.name = "Ene".to_string();
        card.data
            .extra
            .insert("vendor_key".to_string(), serde_json::json!({"keep": 42}));

        save_character_card(&path, &card).expect("save card");

        let entries: Vec<std::ffi::OsString> = std::fs::read_dir(tmp.path())
            .expect("read temp dir")
            .map(|e| e.expect("dir entry").file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from("character.json")],
            "no temp files may be left behind, got {entries:?}"
        );

        let loaded =
            crate::load_character_card(&path.to_string_lossy()).expect("reload card after save");
        assert_eq!(loaded.data.name, "Ene");
        assert_eq!(
            loaded.data.extra.get("vendor_key"),
            Some(&serde_json::json!({"keep": 42}))
        );
    }

    /// Saving over an existing card must replace it atomically (a reader
    /// either sees the old or the new bytes, never a mix) and preserve the
    /// unknown-field catch-all from the original document.
    #[test]
    fn save_character_card_replaces_existing_card_preserving_unknown_fields() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let original = r#"{
            "spec": "chara_card_v3",
            "spec_version": "3.0",
            "data": {
                "name": "Old",
                "description": "",
                "personality": "",
                "scenario": "",
                "mes_example": "",
                "first_mes": "",
                "system_prompt": "",
                "post_history_instructions": "",
                "alternate_greetings": [],
                "tags": [],
                "creator": "",
                "character_version": "",
                "vendor_field": "from-other-app"
            }
        }"#;
        std::fs::write(&path, original).expect("seed existing card");

        let mut card: crate::CharacterCardV3 =
            serde_json::from_str(original).expect("parse seeded card");
        card.data.name = "New".to_string();
        save_character_card(&path, &card).expect("save card");

        let on_disk = std::fs::read_to_string(&path).expect("read back");
        assert_ne!(on_disk, original, "card must be updated in place");
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).expect("valid JSON");
        assert_eq!(
            parsed.pointer("/data/name"),
            Some(&serde_json::json!("New"))
        );
        assert_eq!(
            parsed.pointer("/data/vendor_field"),
            Some(&serde_json::json!("from-other-app")),
            "unknown top-level field must survive the save"
        );
    }
}
