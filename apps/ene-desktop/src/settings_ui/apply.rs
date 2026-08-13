//! Draft apply pipeline: validate → atomic persist → runtime apply →
//! feedback.
//!
//! The apply pipeline funnels a dirty [`SettingsDraft`] to disk and the
//! runtime actor. Async preparation validates (schema subset + plugin
//! `ValidateConfig`); finalize persists atomically and rolls the persisted
//! config back on runtime failure or stale-draft conflict.

use crate::ai_bridge::AiBridge;
#[cfg(test)]
use crate::ai_bridge::AiBridgeError;
use crate::settings::CharacterSettings;

use super::draft::SettingsDraft;

/// The runtime-apply capability the pipeline needs; implemented by
/// [`AiBridge`] and mocked in tests so the rollback path can be exercised
/// without a live actor.
#[cfg(test)]
pub trait SettingsApplier {
    /// Applies a unified settings draft to the runtime actor.
    fn apply_settings_blocking(
        &self,
        request: ene_runtime::SettingsApplyRequest,
    ) -> Result<ene_runtime::SettingsApplyResult, AiBridgeError>;
}

#[cfg(test)]
impl SettingsApplier for AiBridge {
    fn apply_settings_blocking(
        &self,
        request: ene_runtime::SettingsApplyRequest,
    ) -> Result<ene_runtime::SettingsApplyResult, AiBridgeError> {
        AiBridge::apply_settings_blocking(self, request)
    }
}

/// Result of a successful draft apply, ready for UI display.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    /// Revision that was applied (echoes the request revision).
    pub revision: u64,
    /// True when the draft's base revision was stale: nothing was applied.
    pub conflicted: bool,
    /// Sections the runtime actor actually wrote.
    pub applied_sections: Vec<String>,
    /// Impact the runtime reported (hot-reload / plugin restart / app
    /// restart flags).
    pub impact: ene_runtime::SettingsImpact,
    /// Per-section runtime errors that did not abort the apply.
    pub runtime_errors: Vec<String>,
}

impl ApplyOutcome {
    /// Whether the apply reported no runtime errors.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.runtime_errors.is_empty()
    }
}

/// Why a draft apply did not complete.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The runtime apply failed asynchronously (rolled back on disk).
    #[error("runtime apply failed: {0}")]
    Runtime(String),
    /// The proposed config could not be persisted.
    #[error("failed to persist settings: {0}")]
    Persist(String),
}

/// Result of the async preparation phase: the secret-merged config plus any
/// validation errors.
///
/// Deliberately no `Debug` impl: `proposed` holds real secret values after
/// the merge, and a derived impl would let a stray log or assertion print
/// them.
pub struct ApplyPrepare {
    /// Merged (secret-restored) config ready to persist and send.
    pub proposed: ene_config::EneConfig,
    /// Validation errors (schema subset + plugin `ValidateConfig`); empty
    /// means the draft is ready to finalize.
    pub errors: Vec<String>,
}

/// `Debug` never prints the merged config: it holds real secret values.
impl std::fmt::Debug for ApplyPrepare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplyPrepare")
            .field("proposed_sections", &self.proposed.extra.len())
            .field("errors", &self.errors)
            .finish()
    }
}

/// Async preparation: merges secrets, derives local models, runs schema
/// validation on every dirty section, and asks each configured plugin's own
/// validator to check its draft config value. Never blocks the render loop.
pub async fn prepare_apply(
    original: &ene_config::EneConfig,
    editing: &ene_config::EneConfig,
    dirty_paths: &std::collections::BTreeSet<String>,
    handle: std::sync::Arc<ene_runtime::EneHandle>,
) -> ApplyPrepare {
    let mut proposed = build_proposed(original, editing, dirty_paths);
    derive_local_models(&mut proposed);
    let mut errors = Vec::new();

    for path in dirty_paths {
        let section_key = path.split('.').next().unwrap_or(path).to_string();
        super::draft::validate_section(&proposed, &section_key, &mut errors);
    }

    if dirty_paths.contains("plugins")
        && let Ok(plugins) = proposed.get_section::<ene_plugin_host::PluginConfig>()
    {
        let mut names: Vec<&String> = plugins.list.keys().collect();
        names.sort();
        for name in names {
            let value = proposed
                .get_path(&format!("plugins.list.{name}.config"))
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Ok(field_errors) = handle.validate_plugin_config(name, value).await {
                for error in field_errors {
                    errors.push(format!("{name}: {}: {}", error.field_path, error.message));
                }
            }
        }
    }
    ApplyPrepare { proposed, errors }
}

/// Overlays the draft's dirty paths onto a fresh copy of `original`.
///
/// Sections the draft never touched stay verbatim from `original`, so
/// settings written directly to `CharacterSettings` while the settings
/// window is open (graphics, theme, language, mic device, character
/// switch) survive an apply instead of reverting to the window-open
/// snapshot. Values copied from the redacted draft are secret-restored
/// from `original` afterwards.
pub(crate) fn build_proposed(
    original: &ene_config::EneConfig,
    editing: &ene_config::EneConfig,
    dirty_paths: &std::collections::BTreeSet<String>,
) -> ene_config::EneConfig {
    let mut proposed = original.clone();
    for path in dirty_paths {
        overlay_dirty_path(&mut proposed, editing, path);
    }
    merge_value_map(&mut proposed.extra, &original.extra);
    proposed
}

/// Copies one dirty path from `editing` into `proposed`, removing the path
/// when the draft deleted it. Top-level declared fields live outside
/// `extra` and are copied directly.
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
            if let Ok(json) = serde_json::to_string(&value) {
                // `EneConfig::set_path` re-parses the JSON value, so this
                // round-trip preserves the draft value exactly.
                if proposed.set_path(path, &json).is_err() {
                    tracing::warn!(
                        component = "SettingsApply",
                        path,
                        "failed to overlay a dirty config path"
                    );
                }
            }
        }
        None => remove_nested(&mut proposed.extra, &keys),
    }
}

/// Removes a nested value from `extra`; used when a dirty path disappeared
/// from the draft (a section or entry the user deleted).
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

/// Persists `proposed` and pushes it to the runtime actor, rolling back to
/// `original` (the real, pre-apply config) on runtime failure or conflict.
///
/// Synchronous path used by unit tests with fake appliers.
#[cfg(test)]
pub(crate) fn finalize_apply(
    settings: &CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &impl SettingsApplier,
    proposed: ene_config::EneConfig,
    original: ene_config::EneConfig,
) -> Result<ApplyOutcome, ApplyError> {
    settings.set_config(proposed.clone());
    if let Err(e) = settings.save() {
        return Err(ApplyError::Persist(e.to_string()));
    }
    let request = ene_runtime::SettingsApplyRequest {
        revision: draft.revision(),
        base_revision: Some(draft.actor_revision()),
        config: proposed,
    };
    let result = ai
        .apply_settings_blocking(request)
        .map_err(|error| error.to_string());
    apply_outcome_from_result(settings, draft, result, original)
}

/// Persists `proposed` (synchronous file write) and starts the runtime apply
/// as an asynchronous receiver so the render thread never blocks on the
/// actor round-trip. [`finish_finalize`] handles the outcome.
pub(crate) fn begin_finalize(
    settings: &CharacterSettings,
    draft: &SettingsDraft,
    ai: &AiBridge,
    proposed: ene_config::EneConfig,
) -> Result<
    tokio::sync::oneshot::Receiver<Result<ene_runtime::SettingsApplyResult, String>>,
    ApplyError,
> {
    settings.set_config(proposed.clone());
    if let Err(e) = settings.save() {
        return Err(ApplyError::Persist(e.to_string()));
    }
    let request = ene_runtime::SettingsApplyRequest {
        revision: draft.revision(),
        base_revision: Some(draft.actor_revision()),
        config: proposed,
    };
    Ok(ai.apply_settings_async(request))
}

/// Handles the asynchronous runtime-apply outcome, rolling back to
/// `original` on failure or stale-draft conflict.
pub(crate) fn finish_finalize(
    settings: &CharacterSettings,
    draft: &mut SettingsDraft,
    result: Result<ene_runtime::SettingsApplyResult, String>,
    original: ene_config::EneConfig,
) -> Result<ApplyOutcome, ApplyError> {
    apply_outcome_from_result(settings, draft, result, original)
}

fn apply_outcome_from_result(
    settings: &CharacterSettings,
    draft: &mut SettingsDraft,
    result: Result<ene_runtime::SettingsApplyResult, String>,
    original: ene_config::EneConfig,
) -> Result<ApplyOutcome, ApplyError> {
    match result {
        Ok(result) => {
            draft.set_actor_revision(result.current_revision);
            if result.conflicted {
                // The actor rejected the draft as stale *after* we
                // persisted it; roll the disk back to the pre-apply baseline
                // so disk/UI/runtime stay consistent, and keep the edits for
                // the user to re-apply.
                settings.set_config(original.clone());
                if let Err(rollback_error) = settings.save() {
                    tracing::error!(
                        component = "SettingsApply",
                        error = %rollback_error,
                        "conflict rollback persist failed"
                    );
                }
                draft.resync_baseline(settings.config());
                return Ok(ApplyOutcome {
                    revision: result.revision,
                    conflicted: true,
                    applied_sections: Vec::new(),
                    impact: ene_runtime::SettingsImpact::default(),
                    runtime_errors: Vec::new(),
                });
            }
            draft.mark_applied();
            Ok(ApplyOutcome {
                revision: result.revision,
                conflicted: result.conflicted,
                applied_sections: result.applied_sections.into_iter().collect(),
                impact: result.impact,
                runtime_errors: result.errors,
            })
        }
        Err(error) => {
            // Roll back the persisted layer so disk and UI agree; the actor
            // may have partially reacted, but the next apply diffs against
            // the actor's own config and reconciles.
            settings.set_config(original);
            if let Err(rollback_error) = settings.save() {
                tracing::error!(
                    component = "SettingsApply",
                    error = %rollback_error,
                    "rollback persist failed after a failed runtime apply"
                );
            }
            draft.resync(settings.config());
            Err(ApplyError::Runtime(error))
        }
    }
}

/// Restores real secret values into a redacted draft config.
///
/// Every leaf equal to [`super::draft::SECRET_PLACEHOLDER`] is replaced by the original
/// value at the same path; a replacement string the user typed wins; `null`
/// stays `null` (an explicit deletion). The walk mirrors the draft's
/// redaction rules, including all array elements.
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

/// Derives `ai.local_models` from `plugins.list.{local-llm,llama-server}`
/// profiles, keeping plugin profiles the single UI source of truth for local
/// models while the runtime's local-model resolution keeps reading
/// `ai.local_models`. The map is regenerated from scratch on every apply, so
/// a removed profile disappears from it automatically.
fn derive_local_models(config: &mut ene_config::EneConfig) {
    let Ok(mut ai) = config.get_section::<ene_ai::AiConfig>() else {
        return;
    };
    ai.local_models = local_model_defs_from_plugins(config);
    drop(config.set_section(&ai));
}

/// Builds the `ai.local_models` map from `plugins.list.{local-llm,
/// llama-server}` profiles. Shared by the apply pipeline and the AI page so
/// both resolve local models identically.
#[must_use]
pub fn local_model_defs_from_plugins(
    config: &ene_config::EneConfig,
) -> std::collections::BTreeMap<String, ene_ai::LocalModelDef> {
    let mut local_models = std::collections::BTreeMap::new();
    if let Ok(plugins) = config.get_section::<ene_plugin_host::PluginConfig>() {
        for plugin in ["local-llm", "llama-server"] {
            let Some(entry) = plugins.list.get(plugin) else {
                continue;
            };
            for (name, profile) in &entry.profiles {
                let Some(object) = profile.as_object() else {
                    continue;
                };
                let gpu_layers = object
                    .get("gpu_layers")
                    .and_then(serde_json::Value::as_i64)
                    .map_or_else(|| "auto".to_string(), |layers| layers.to_string());
                let context_size = object
                    .get("context_size")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|size| u32::try_from(size).ok())
                    .unwrap_or(16_384);
                let dimensions = object
                    .get("dimensions")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|size| usize::try_from(size).ok());
                local_models.insert(
                    name.clone(),
                    ene_ai::LocalModelDef {
                        url: object
                            .get("url")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        artifact_id: object
                            .get("artifact_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        artifact_version: object
                            .get("artifact_version")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        quantization: object
                            .get("quantization")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        model_path: object
                            .get("model_path")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        gpu_layers,
                        context_size,
                        dimensions,
                    },
                );
            }
        }
    }
    local_models
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CharacterSettings;
    use serde_json::json;

    /// Always-failing applier: the actor side is unreachable in a unit test,
    /// and the pipeline must still roll the persisted config back.
    struct DeadApplier;

    impl SettingsApplier for DeadApplier {
        fn apply_settings_blocking(
            &self,
            _request: ene_runtime::SettingsApplyRequest,
        ) -> Result<ene_runtime::SettingsApplyResult, AiBridgeError> {
            Err(AiBridgeError::Runtime(
                ene_runtime::EneRuntimeError::ChannelClosed,
            ))
        }
    }

    /// Applier that always reports a stale-draft conflict.
    struct ConflictingApplier;

    impl SettingsApplier for ConflictingApplier {
        fn apply_settings_blocking(
            &self,
            request: ene_runtime::SettingsApplyRequest,
        ) -> Result<ene_runtime::SettingsApplyResult, AiBridgeError> {
            Ok(ene_runtime::SettingsApplyResult {
                revision: request.revision,
                current_revision: request.base_revision.unwrap_or(0).saturating_add(1),
                conflicted: true,
                applied_sections: std::collections::BTreeSet::new(),
                impact: ene_runtime::SettingsImpact::default(),
                errors: Vec::new(),
            })
        }
    }

    fn test_settings() -> CharacterSettings {
        let tmp = std::env::temp_dir().join(format!("ene-apply-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("test temp dir");
        let settings = CharacterSettings::discover(&tmp, "Alicia");
        settings.save().expect("test settings persist");
        settings
    }

    /// The full apply pipeline round-trips through a real actor, and a dead
    /// actor rolls the persisted config back so the UI never shows a
    /// half-applied state.
    #[test]
    fn runtime_apply_failure_rolls_back_persisted_config() {
        let settings = test_settings();
        let original_config = settings.config();
        let original = settings
            .config_section::<ene_mind::MindConfig>()
            .session
            .session_timeout_minutes;
        let mut draft = SettingsDraft::new(settings.config());
        let mut mind = draft.section::<ene_mind::MindConfig>();
        mind.session.session_timeout_minutes = original.saturating_add(7);
        draft.set_section(&mind);

        let mut proposed = build_proposed(&original_config, draft.editing(), draft.dirty_paths());
        derive_local_models(&mut proposed);
        let result = finalize_apply(
            &settings,
            &mut draft,
            &DeadApplier,
            proposed,
            original_config,
        );
        assert!(result.is_err(), "a dead actor must fail the apply");
        assert_eq!(
            settings
                .config_section::<ene_mind::MindConfig>()
                .session
                .session_timeout_minutes,
            original,
            "the persisted config must be rolled back"
        );
        assert!(
            !draft.is_dirty(),
            "the draft resyncs to the rolled-back baseline"
        );
    }

    #[test]
    fn local_models_are_derived_from_plugin_profiles() {
        let mut config = ene_config::EneConfig::default();
        let mut plugins = ene_plugin_host::PluginConfig::default();
        plugins
            .list
            .entry("local-llm".to_string())
            .or_default()
            .profiles
            .insert(
                "jina".to_string(),
                json!({
                    "url": "https://example.com/jina.gguf",
                    "quantization": "F16",
                    "gpu_layers": 33,
                    "context_size": 8192,
                    "dimensions": 1024
                }),
            );
        drop(config.set_section(&plugins));

        let map = local_model_defs_from_plugins(&config);
        let def = map.get("jina").expect("profile derives a model entry");
        assert_eq!(def.url, "https://example.com/jina.gguf");
        assert_eq!(def.quantization, "F16");
        assert_eq!(def.gpu_layers, "33");
        assert_eq!(def.context_size, 8192);
        assert_eq!(def.dimensions, Some(1024));
        assert!(
            !map.contains_key("ghost"),
            "only configured profiles appear"
        );
    }

    /// Direct writes to `CharacterSettings` while the window is open
    /// (graphics, theme, language, mic device, character switch) land in
    /// sections the draft never touched; an apply must keep them instead of
    /// reverting to the window-open snapshot.
    #[test]
    fn build_proposed_overlays_only_dirty_paths() {
        let mut original = ene_config::EneConfig {
            user_name: "Old Name".to_string(),
            ..ene_config::EneConfig::default()
        };
        drop(original.set_path("desktop.theme", r#""light""#));
        drop(original.set_path("ai.tasks.chat.model", r#""gpt-original""#));

        let mut editing = original.clone();
        // The draft was created before the theme was switched to light, so
        // its `desktop` section is stale.
        drop(editing.set_path("desktop.theme", r#""system""#));
        editing.user_name = "New Name".to_string();
        drop(editing.set_path("ai.tasks.chat.model", r#""gpt-edited""#));

        let dirty = std::collections::BTreeSet::from(["user_name".to_string(), "ai".to_string()]);
        let proposed = build_proposed(&original, &editing, &dirty);

        assert_eq!(proposed.user_name, "New Name");
        assert_eq!(
            proposed.get_path("ai.tasks.chat.model"),
            Some(serde_json::json!("gpt-edited")),
            "dirty paths are overlaid from the draft"
        );
        assert_eq!(
            proposed.get_path("desktop.theme"),
            Some(serde_json::json!("light")),
            "a direct write outside the dirty paths must survive"
        );
    }

    /// Secrets redacted into the draft's dirty sections are restored from
    /// the original config, exactly like the full-config merge path.
    #[test]
    fn build_proposed_restores_secrets_in_overlaid_sections() {
        let mut original = ene_config::EneConfig::default();
        drop(original.set_section_value(
            "plugins",
            serde_json::json!({
                "list": {
                    "demo": {
                        "enable": true,
                        "config": {
                            "api_key": "sk-stored",
                            "voice": "af_original"
                        }
                    }
                }
            }),
        ));

        let mut editing = original.clone();
        drop(editing.set_section_value(
            "plugins",
            serde_json::json!({
                "list": {
                    "demo": {
                        "enable": true,
                        "config": {
                            "api_key": super::super::draft::SECRET_PLACEHOLDER,
                            "voice": "af_heart"
                        }
                    }
                }
            }),
        ));

        let dirty = std::collections::BTreeSet::from(["plugins".to_string()]);
        let proposed = build_proposed(&original, &editing, &dirty);
        assert_eq!(
            proposed.get_path("plugins.list.demo.config.api_key"),
            Some(serde_json::json!("sk-stored")),
            "the placeholder must be replaced by the stored secret"
        );
        assert_eq!(
            proposed.get_path("plugins.list.demo.config.voice"),
            Some(serde_json::json!("af_heart")),
            "the user's edit wins"
        );
    }

    /// A dirty path that disappeared from the draft is removed from the
    /// proposed config instead of being resurrected from the original.
    #[test]
    fn build_proposed_removes_paths_deleted_from_the_draft() {
        let mut original = ene_config::EneConfig::default();
        drop(original.set_section_value(
            "plugins",
            serde_json::json!({ "list": { "demo": { "enable": true } } }),
        ));
        let mut editing = original.clone();
        let _ = editing.remove_section("plugins");

        let dirty = std::collections::BTreeSet::from(["plugins".to_string()]);
        let proposed = build_proposed(&original, &editing, &dirty);
        assert!(
            proposed.section_value("plugins").is_none(),
            "a section deleted from the draft must not reappear"
        );
    }

    #[test]
    fn stale_draft_conflict_rolls_back_disk_and_keeps_edits() {
        let settings = test_settings();
        let original_config = settings.config();
        let original = settings
            .config_section::<ene_mind::MindConfig>()
            .session
            .session_timeout_minutes;
        let mut draft = SettingsDraft::new(settings.config());
        let mut mind = draft.section::<ene_mind::MindConfig>();
        mind.session.session_timeout_minutes = original.saturating_add(99);
        draft.set_section(&mind);
        let revision = draft.revision();

        let mut proposed = build_proposed(&original_config, draft.editing(), draft.dirty_paths());
        derive_local_models(&mut proposed);
        let outcome = finalize_apply(
            &settings,
            &mut draft,
            &ConflictingApplier,
            proposed,
            original_config,
        )
        .expect("conflict is a result");
        assert!(outcome.conflicted);
        assert!(
            draft.is_dirty(),
            "the user's edits survive a conflict for review"
        );
        assert_eq!(draft.revision(), revision);
        assert_eq!(
            settings
                .config_section::<ene_mind::MindConfig>()
                .session
                .session_timeout_minutes,
            original,
            "the disk is rolled back to the pre-apply baseline"
        );
    }

    #[test]
    fn merge_restores_unchanged_secrets_and_keeps_replacements_and_deletes() {
        let original = super::super::draft::config_with_real_secrets();
        let mut draft = SettingsDraft::new(original.clone());
        // Replacement: the user typed a new inline key.
        draft.set_path(
            "ai.providers.openai.api_key.inline",
            json!("sk-user-replacement"),
        );
        // Deletion: the credential entry is removed from the map.
        let mut credentials = draft
            .editing()
            .get_path("plugins.list.demo.credentials")
            .expect("credentials map exists");
        credentials
            .as_object_mut()
            .expect("credentials object")
            .remove("cred-key");
        draft.set_path("plugins.list.demo.credentials", credentials);
        // Everything else stays redacted (unchanged) in the draft.

        let merged = merge_secrets(&original, draft.editing());
        let ai = merged
            .get_section::<ene_ai::AiConfig>()
            .expect("ai section");
        assert_eq!(
            ai.providers["openai"].api_key.inline, "sk-user-replacement",
            "a user replacement wins over the stored secret"
        );
        let plugins = merged
            .get_section::<ene_plugin_host::PluginConfig>()
            .expect("plugins section");
        assert_eq!(
            plugins.list["demo"].credentials.get("cred-key"),
            None,
            "an explicit deletion stays deleted"
        );
        assert_eq!(
            plugins.list["demo"].config["token"], "config-stored-token",
            "an unchanged secret is restored from the store"
        );
        let merged_text = serde_json::to_string(&merged).expect("merged serializes");
        assert!(
            !merged_text.contains(super::super::draft::SECRET_PLACEHOLDER),
            "no placeholder may reach the persisted config"
        );
    }
}
