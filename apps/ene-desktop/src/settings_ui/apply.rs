//! Draft apply pipeline: validate local `desktop.*`, persist the desktop
//! config, and PATCH dirty core sections through `ene-api`.
use std::collections::BTreeSet;

use crate::core_session::CoreSession;
use crate::settings::CharacterSettings;

use super::draft::{FieldImpact, SettingsDraft};

/// Local impact classification shown on the apply banner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsImpact {
    pub runtime_reload: bool,
    pub plugin_restart: bool,
    pub app_restart: bool,
}

impl SettingsImpact {
    #[must_use]
    pub fn from_sections(sections: &BTreeSet<String>) -> Self {
        let mut impact = Self::default();
        for key in sections {
            match SettingsDraft::default_impact_for(key) {
                FieldImpact::PluginRestart => impact.plugin_restart = true,
                FieldImpact::AppRestart => impact.app_restart = true,
                FieldImpact::RuntimeReload => impact.runtime_reload = true,
                FieldImpact::Immediate => {}
            }
        }
        impact
    }
}

/// Result of a successful draft apply, ready for UI display.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub revision: u64,
    pub conflicted: bool,
    pub applied_sections: Vec<String>,
    pub impact: SettingsImpact,
    pub runtime_errors: Vec<String>,
}

impl ApplyOutcome {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.runtime_errors.is_empty()
    }
}

/// Why a draft apply did not complete.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The core PATCH failed (local desktop persist was rolled back).
    #[error("runtime apply failed: {0}")]
    Runtime(String),
    /// The proposed config could not be persisted.
    #[error("failed to persist settings: {0}")]
    Persist(String),
}

/// Result of the async preparation phase.
///
/// Deliberately no `Debug` impl: `proposed` may hold secret values after
/// the merge, and a derived impl would let a stray log print them.
pub struct ApplyPrepare {
    pub proposed: ene_config::EneConfig,
    pub errors: Vec<String>,
}

impl std::fmt::Debug for ApplyPrepare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplyPrepare")
            .field("proposed_sections", &self.proposed.extra.len())
            .field("errors", &self.errors)
            .finish()
    }
}

/// Merges secrets, overlays dirty paths, and validates the local `desktop`
/// schema. Core sections are opaque JSON and are not schema-checked here.
pub fn prepare_apply(
    original: &ene_config::EneConfig,
    editing: &ene_config::EneConfig,
    dirty_paths: &BTreeSet<String>,
) -> ApplyPrepare {
    let proposed = merge_secrets(original, &build_proposed(original, editing, dirty_paths));
    let mut errors = Vec::new();
    for path in dirty_paths {
        let section_key = path.split('.').next().unwrap_or(path);
        if section_key == "desktop" {
            super::draft::validate_section(&proposed, section_key, &mut errors);
        }
    }
    ApplyPrepare { proposed, errors }
}

/// Overlays the draft's dirty paths onto a fresh copy of `original`.
pub(crate) fn build_proposed(
    original: &ene_config::EneConfig,
    editing: &ene_config::EneConfig,
    dirty_paths: &BTreeSet<String>,
) -> ene_config::EneConfig {
    let mut proposed = original.clone();
    for path in dirty_paths {
        overlay_dirty_path(&mut proposed, editing, path);
    }
    merge_value_map(&mut proposed.extra, &original.extra);
    proposed
}

fn overlay_dirty_path(
    proposed: &mut ene_config::EneConfig,
    editing: &ene_config::EneConfig,
    path: &str,
) {
    match path {
        "character" => {
            proposed.character.clone_from(&editing.character);
            return;
        }
        "user_name" => {
            proposed.user_name.clone_from(&editing.user_name);
            return;
        }
        "runtime_rules" => {
            proposed.runtime_rules.clone_from(&editing.runtime_rules);
            return;
        }
        "user_persona" => {
            proposed.user_persona.clone_from(&editing.user_persona);
            return;
        }
        _ => {}
    }
    let keys: Vec<&str> = path.split('.').filter(|key| !key.is_empty()).collect();
    if keys.is_empty() {
        return;
    }
    match editing.get_path(path) {
        Some(value) => {
            if let Ok(json) = serde_json::to_string(&value)
                && proposed.set_path(path, &json).is_err()
            {
                tracing::warn!(
                    component = "SettingsApply",
                    path,
                    "failed to overlay a dirty config path"
                );
            }
        }
        None => remove_nested(&mut proposed.extra, &keys),
    }
}

fn remove_nested(extra: &mut indexmap::IndexMap<String, serde_json::Value>, keys: &[&str]) {
    if keys.len() == 1 {
        extra.shift_remove(keys[0]);
        return;
    }
    let Some(serde_json::Value::Object(parent)) = extra.get_mut(keys[0]) else {
        return;
    };
    let mut current = parent;
    for key in &keys[1..keys.len() - 1] {
        let Some(serde_json::Value::Object(next)) = current.get_mut(*key) else {
            return;
        };
        current = next;
    }
    current.remove(keys[keys.len() - 1]);
}

/// Desktop process persists only `desktop.*` plus declared top-level fields.
#[must_use]
pub fn local_config(proposed: &ene_config::EneConfig) -> ene_config::EneConfig {
    let mut local = proposed.clone();
    local.extra.retain(|key, _| key == "desktop");
    local
}

/// Top-level extra keys other than `desktop` that the draft dirtied.
#[must_use]
pub fn core_patch(
    proposed: &ene_config::EneConfig,
    dirty_paths: &BTreeSet<String>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for path in dirty_paths {
        let key = path.split('.').next().unwrap_or(path);
        if key == "desktop"
            || matches!(
                key,
                "character" | "user_name" | "runtime_rules" | "user_persona" | "$schema"
            )
        {
            continue;
        }
        keys.insert(key.to_owned());
    }
    for key in keys {
        if let Some(value) = proposed.section_value(&key) {
            map.insert(key, value);
        }
    }
    serde_json::Value::Object(map)
}

/// Persists the local desktop config and starts the core PATCH.
pub(crate) fn begin_finalize(
    settings: &CharacterSettings,
    draft: &SettingsDraft,
    ai: &CoreSession,
    proposed: ene_config::EneConfig,
) -> Result<tokio::sync::oneshot::Receiver<Result<BTreeSet<String>, String>>, ApplyError> {
    settings.set_config(local_config(&proposed));
    if let Err(error) = settings.save() {
        return Err(ApplyError::Persist(error.to_string()));
    }
    let patch = core_patch(&proposed, draft.dirty_paths());
    Ok(ai.apply_settings_async(patch))
}

/// Handles the asynchronous core PATCH outcome, rolling back local disk
/// on failure.
pub(crate) fn finish_finalize(
    settings: &CharacterSettings,
    draft: &mut SettingsDraft,
    result: Result<BTreeSet<String>, String>,
    original: ene_config::EneConfig,
) -> Result<ApplyOutcome, ApplyError> {
    match result {
        Ok(applied) => {
            draft.mark_applied();
            Ok(ApplyOutcome {
                revision: draft.revision(),
                conflicted: false,
                applied_sections: applied.iter().cloned().collect(),
                impact: SettingsImpact::from_sections(&applied),
                runtime_errors: Vec::new(),
            })
        }
        Err(error) => {
            settings.set_config(original);
            if let Err(rollback_error) = settings.save() {
                tracing::error!(
                    component = "SettingsApply",
                    error = %rollback_error,
                    "rollback persist failed after a failed core settings patch"
                );
            }
            draft.resync(settings.config());
            Err(ApplyError::Runtime(error))
        }
    }
}

/// Restores real secret values into a redacted draft config.
pub(crate) fn merge_secrets(
    original: &ene_config::EneConfig,
    editing: &ene_config::EneConfig,
) -> ene_config::EneConfig {
    let mut merged = editing.clone();
    merge_value_map(&mut merged.extra, &original.extra);
    merged
}

fn merge_value_map(
    merged: &mut indexmap::IndexMap<String, serde_json::Value>,
    original: &indexmap::IndexMap<String, serde_json::Value>,
) {
    for (key, value) in merged.iter_mut() {
        let original_value = original.get(key);
        match value {
            serde_json::Value::String(string) if string == super::draft::SECRET_PLACEHOLDER => {
                if let Some(original_value) = original_value {
                    *value = original_value.clone();
                } else {
                    *value = serde_json::Value::String(String::new());
                }
            }
            serde_json::Value::Object(object) => {
                if let Some(original_object) = original_value.and_then(serde_json::Value::as_object)
                {
                    merge_object_value(key, object, original_object);
                }
            }
            serde_json::Value::Array(items) => {
                let original_items = original_value.and_then(serde_json::Value::as_array);
                for (index, item) in items.iter_mut().enumerate() {
                    match item {
                        serde_json::Value::String(string)
                            if string == super::draft::SECRET_PLACEHOLDER =>
                        {
                            if let Some(original_item) =
                                original_items.and_then(|items| items.get(index))
                            {
                                *item = original_item.clone();
                            }
                        }
                        serde_json::Value::Object(_) => {
                            if let Some(original_item) =
                                original_items.and_then(|items| items.get(index))
                                && let Some(original_object) = original_item.as_object()
                                && let Some(object) = item.as_object_mut()
                            {
                                merge_object_value(key, object, original_object);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn merge_object_value(
    key: &str,
    merged: &mut serde_json::Map<String, serde_json::Value>,
    original: &serde_json::Map<String, serde_json::Value>,
) {
    if key == "credentials" {
        for (credential_key, value) in merged.iter_mut() {
            if value.as_str() == Some(super::draft::SECRET_PLACEHOLDER)
                && let Some(original_value) = original.get(credential_key)
            {
                *value = original_value.clone();
            }
        }
        return;
    }
    if key == "api_key" {
        if let Some(inline) = merged.get_mut("inline")
            && inline.as_str() == Some(super::draft::SECRET_PLACEHOLDER)
            && let Some(original_value) = original.get("inline")
        {
            *inline = original_value.clone();
        }
        return;
    }
    for (child_key, value) in merged.iter_mut() {
        match value {
            serde_json::Value::String(string) if string == super::draft::SECRET_PLACEHOLDER => {
                if let Some(original_value) = original.get(child_key) {
                    *value = original_value.clone();
                }
            }
            serde_json::Value::Object(object) => {
                if let Some(original_child) = original
                    .get(child_key)
                    .and_then(serde_json::Value::as_object)
                {
                    merge_object_value(child_key, object, original_child);
                }
            }
            serde_json::Value::Array(items) => {
                let original_items = original
                    .get(child_key)
                    .and_then(serde_json::Value::as_array);
                for (index, item) in items.iter_mut().enumerate() {
                    if let serde_json::Value::String(string) = item
                        && string == super::draft::SECRET_PLACEHOLDER
                        && let Some(original_item) =
                            original_items.and_then(|items| items.get(index))
                    {
                        *item = original_item.clone();
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn config_with_sections() -> ene_config::EneConfig {
        let mut config = ene_config::EneConfig::default();
        drop(config.set_section_value("desktop", json!({"language": "en", "theme": "dark"})));
        drop(config.set_section_value(
            "ai",
            json!({"tasks": {"chat": {"plugin": "provider.openai_compat"}}}),
        ));
        config
    }

    #[test]
    fn local_config_keeps_only_desktop_extra() {
        let local = local_config(&config_with_sections());
        assert!(local.extra.contains_key("desktop"));
        assert!(!local.extra.contains_key("ai"));
    }

    #[test]
    fn core_patch_skips_desktop_and_declared_fields() {
        let proposed = config_with_sections();
        let dirty = BTreeSet::from([
            "desktop.theme".to_owned(),
            "ai".to_owned(),
            "user_name".to_owned(),
        ]);
        let patch = core_patch(&proposed, &dirty);
        let object = patch.as_object().expect("object patch");
        assert!(object.contains_key("ai"));
        assert!(!object.contains_key("desktop"));
        assert!(!object.contains_key("user_name"));
    }

    #[test]
    fn build_proposed_overlays_dirty_extra() {
        let original = config_with_sections();
        let mut editing = original.clone();
        drop(editing.set_section_value(
            "ai",
            json!({"tasks": {"chat": {"plugin": "provider.openai_compat"}}}),
        ));
        let dirty = BTreeSet::from(["ai".to_owned()]);
        let proposed = build_proposed(&original, &editing, &dirty);
        assert_eq!(
            proposed
                .section_value("ai")
                .and_then(|value| value.pointer("/tasks/chat/plugin").cloned()),
            Some(json!("provider.openai_compat"))
        );
    }

    #[test]
    fn impact_from_sections_flags_runtime_reload() {
        let sections = BTreeSet::from(["ai".to_owned(), "desktop".to_owned()]);
        let impact = SettingsImpact::from_sections(&sections);
        assert!(impact.runtime_reload);
        assert!(!impact.app_restart);
    }

    #[test]
    fn merge_secrets_restores_placeholder() {
        let mut original = ene_config::EneConfig::default();
        drop(original.set_section_value("ai", json!({"tasks": {"chat": {"api_key": "sk-real"}}})));
        let mut editing = original.clone();
        drop(editing.set_section_value(
            "ai",
            json!({"tasks": {"chat": {"api_key": super::super::draft::SECRET_PLACEHOLDER}}}),
        ));
        let merged = merge_secrets(&original, &editing);
        assert_eq!(
            merged
                .get_path("ai.tasks.chat.api_key")
                .and_then(|value| value.as_str().map(str::to_owned)),
            Some("sk-real".to_owned())
        );
    }
}
