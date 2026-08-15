//! Shared schema-driven provider config form used by the Voice page (and
//! reusable by Engines / Plugins).
//!
//! The Voice page edits only `ai.tts.provider` / `ai.stt.provider` (routing);
//! every provider-owned value lives in `plugins.list.<plugin>.config`, which
//! this module renders from the plugin's advertised JSON Schema with dynamic
//! option loading (`ListConfigOptions`) and null-as-empty-object handling.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ai_bridge::AiBridge;
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::input::SettingsInputState;
use crate::settings_ui::schema_form::{SchemaFormOptions, schema_object_form};
use ene_plugin_host::PluginSettingsSnapshot;

pub(crate) const BUILTIN_PROVIDER_I18N_IDS: &[(&str, &str)] = &[
    ("kokoro", "kokoro"),
    ("voicevox", "voicevox"),
    ("whisper", "whisper"),
    ("edge-tts", "edge-tts"),
    ("openai_tts", "openai-tts"),
    ("elevenlabs", "elevenlabs"),
];

fn provider_i18n_id(kind: &str) -> Option<&'static str> {
    BUILTIN_PROVIDER_I18N_IDS
        .iter()
        .find_map(|(provider_kind, i18n_id)| (*provider_kind == kind).then_some(*i18n_id))
}

/// Maps a provider kind (the `ai.tts.provider` / `ai.stt.provider` value,
/// also the factory key) to the plugin list name owning its config.
///
/// `openai_tts` is the only built-in kind that differs from its plugin name;
/// unknown kinds are assumed to name their plugin directly.
#[must_use]
pub fn plugin_name_for_provider_kind(kind: &str) -> String {
    if kind == "openai_tts" {
        "openai-tts".to_string()
    } else {
        kind.to_string()
    }
}

/// Localized provider selector label (Voice page combo). Built-in kinds map
/// onto `provider-selector-<kind>-label` FTL keys; third-party kinds fall
/// back to the raw kind string.
#[must_use]
pub fn provider_display_name(kind: &str) -> String {
    if let Some(i18n_id) = provider_i18n_id(kind) {
        fl(&format!("provider-selector-{i18n_id}-label"))
    } else {
        kind.to_string()
    }
}

/// Localized provider selector description (combo hover text); `None` for
/// third-party kinds without metadata.
#[must_use]
pub fn provider_description(kind: &str) -> Option<String> {
    provider_i18n_id(kind).map(|i18n_id| fl(&format!("provider-selector-{i18n_id}-desc")))
}

/// Localized provider group (e.g. Local / Cloud) shown next to the
/// selector. Third-party kinds have no group metadata.
#[must_use]
pub fn provider_display_group(kind: &str) -> Option<String> {
    const LOCAL: &[&str] = &["kokoro", "voicevox", "whisper"];
    const CLOUD: &[&str] = &["edge-tts", "openai_tts", "elevenlabs"];
    if LOCAL.contains(&kind) {
        Some(fl("provider-selector-group-local"))
    } else if CLOUD.contains(&kind) {
        Some(fl("provider-selector-group-cloud"))
    } else {
        None
    }
}

/// Dotted config path for a plugin's provider-owned settings.
#[must_use]
pub fn provider_config_path(plugin: &str) -> String {
    format!("plugins.list.{plugin}.config")
}

/// Renders the schema-driven config form for one provider's plugin config
/// (`plugins.list.<plugin>.config`), creating the blob as an empty object on
/// first edit. Returns `true` when the draft changed.
///
/// `snapshots` is the live plugin settings snapshot list; when the plugin is
/// not running (no snapshot), the caller renders the enable hint instead.
pub fn render_provider_config_form(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshots: &[PluginSettingsSnapshot],
    plugin: &str,
    provider_kind: &str,
) -> bool {
    let Some(snapshot) = snapshots.iter().find(|snapshot| snapshot.id == plugin) else {
        return false;
    };
    let Some(schema) = &snapshot.schema else {
        return false;
    };
    let schema_path = provider_config_path(plugin);
    let mut config_value = draft
        .editing()
        .get_path(&schema_path)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if config_value.is_null() {
        config_value = serde_json::Value::Object(serde_json::Map::new());
    }
    let options = schema_form_options(ui, ai, input, snapshot, provider_kind);
    schema_object_form(
        ui,
        schema,
        &mut config_value,
        &schema_path,
        SchemaFormOptions {
            show_advanced: true,
            show_impact: true,
            epoch: draft.applied_revision(),
            options: Some(&options),
        },
    )
    .then(|| draft.set_path(&schema_path, config_value))
    .is_some()
}

/// Dynamic options for `x-ene-ui.options_path` fields, fetched once per
/// plugin and polled every frame (same contract as the Engines/Plugins
/// pages).
pub fn schema_form_options(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
    provider_kind: &str,
) -> BTreeMap<String, Vec<(String, String)>> {
    let Some(schema) = &snapshot.schema else {
        return BTreeMap::new();
    };
    let state = input.plugin_options.entry(snapshot.id.clone()).or_default();
    state.poll();
    let mut fields = Vec::new();
    collect_options_paths(schema, "", &mut fields);
    if !fields.is_empty() {
        // Explicit "load candidates" operation: fetching dynamic options
        // may query a live engine (VOICEVOX /speakers, ElevenLabs voices),
        // so it must never happen just by opening the page — and it must
        // never start a managed engine. After a successful or failed load,
        // the button becomes "reload" and re-runs the fetch with the latest
        // state.
        if !state.loading() {
            let finished = state.data.is_some() || state.error.is_some();
            let label = if finished {
                fl("engines-reload-candidates")
            } else {
                fl("engines-load-candidates")
            };
            if ui.small_button(label).clicked() {
                let plugin = snapshot.id.clone();
                if finished {
                    state.restart(ai.fetch_plugin_options(plugin, fields));
                } else {
                    state.start(ai.fetch_plugin_options(plugin, fields));
                }
            }
        }
    }
    if state.loading() {
        ui.weak(fl("plugins-loading"));
    }
    if let Some(error) = state.error.clone() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let mut options = state.data.clone().unwrap_or_default();
    merge_static_capability_candidates(snapshot, provider_kind, &mut options);
    if !options.is_empty() {
        return options;
    }
    BTreeMap::new()
}

/// Merges the provider's statically advertised voices/models (from the
/// handshake capabilities) into the option map, so fields like Kokoro's
/// `voice` get a picker without a live engine query. Dynamic options (when
/// loaded) win for the same path.
fn merge_static_capability_candidates(
    snapshot: &PluginSettingsSnapshot,
    provider_kind: &str,
    options: &mut BTreeMap<String, Vec<(String, String)>>,
) {
    let tts = snapshot
        .capabilities
        .tts_providers
        .iter()
        .find(|spec| spec.kind == provider_kind);
    if let Some(tts) = tts
        && !tts.voices.is_empty()
    {
        let voices = &tts.voices;
        // ElevenLabs names its field `voice_id`; every other TTS plugin uses
        // `voice`.
        let voice_path = if provider_kind == "elevenlabs" {
            "voice_id"
        } else {
            "voice"
        };
        let entry = options.entry(voice_path.to_string()).or_default();
        if entry.is_empty() {
            entry.extend(voices.iter().map(|voice| (voice.clone(), voice.clone())));
        }
    }
    let stt = snapshot
        .capabilities
        .stt_providers
        .iter()
        .find(|spec| spec.kind == provider_kind);
    if let Some(stt) = stt
        && !stt.models.is_empty()
    {
        let entry = options.entry("model".to_string()).or_default();
        if entry.is_empty() {
            entry.extend(
                stt.models
                    .iter()
                    .map(|model| (model.clone(), model.clone())),
            );
        }
    }
}

/// Renders the shared artifact card for a selected local provider (Voice
/// page): installed/catalog version, download/update, progress, cancel,
/// rollback, and uninstall — the same component the Engines page uses.
pub fn render_provider_artifact_card(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    artifacts: &[ene_plugin_host::ArtifactSnapshot],
    provider_kind: &str,
) {
    let artifact_id = match provider_kind {
        "voicevox" => "voicevox-engine",
        "whisper" => "whisper-server",
        "llama-server" => "llama-server",
        _ => return,
    };
    crate::settings_ui::artifact_card::render_artifact_card(ui, ai, input, artifacts, artifact_id);
}

/// Shared provider config actions used by the Voice and Engines pages:
/// plugin status, an explicit enable affordance for stopped plugins, and
/// the plugin's own `ValidateConfig` operation with result display.
///
/// `snapshot` may be absent (plugin not configured/not running); the
/// actions still offer the enable affordance.
pub fn render_config_actions(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: Option<&PluginSettingsSnapshot>,
    plugin: &str,
) {
    use crate::settings_ui::components::{BadgeTone, status_badge};
    use ene_plugin_host::PluginHealthState;

    let enabled_key = format!("plugins.list.{plugin}.enable");
    let enabled = draft
        .editing()
        .get_path(&enabled_key)
        .and_then(|value| value.as_bool())
        .unwrap_or_else(|| snapshot.is_some_and(|snapshot| snapshot.enabled));
    ui.horizontal_wrapped(|ui| {
        match snapshot.map(|snapshot| snapshot.health) {
            Some(PluginHealthState::Running) => {
                status_badge(ui, &fl("plugins-health-running"), BadgeTone::Ok);
            }
            Some(PluginHealthState::Disabled) => {
                status_badge(ui, &fl("plugins-health-disabled"), BadgeTone::Error);
            }
            Some(PluginHealthState::RequirementsUnmet) => {
                status_badge(
                    ui,
                    &fl("plugins-health-requirements_unmet"),
                    BadgeTone::Warn,
                );
            }
            Some(PluginHealthState::Stopped) | None => {
                status_badge(ui, &fl("plugins-health-stopped"), BadgeTone::Neutral);
            }
        }
        if !enabled
            && ui
                .small_button(fl("plugins-enable"))
                .on_hover_text(fl("plugins-enable-hint"))
                .clicked()
        {
            draft.set_path(&enabled_key, serde_json::Value::Bool(true));
        }
        if snapshot.is_some_and(|snapshot| snapshot.supports_validate_config) {
            let validate_clicked = ui
                .small_button(fl("plugins-validate"))
                .on_hover_text(fl("plugins-validate-hint"))
                .clicked();
            let state = input
                .plugin_validation
                .entry(plugin.to_string())
                .or_default();
            state.poll();
            if validate_clicked && !state.loading() {
                let merged = super::apply::merge_secrets(draft.persisted(), draft.editing());
                let draft_value = merged
                    .get_path(&provider_config_path(plugin))
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                // Re-validate with the latest draft every click: a cached
                // result must not make later clicks no-ops.
                state.restart(ai.validate_plugin_config_async(plugin.to_string(), draft_value));
            }
            if state.loading() {
                ui.weak(fl("plugins-loading"));
            } else if let Some(errors) = state.data.clone() {
                if errors.is_empty() {
                    ui.weak(fl("plugins-validation-ok"));
                } else {
                    ui.colored_label(ui.visuals().warn_fg_color, fl("plugins-validation-errors"));
                    for error in &errors {
                        ui.weak(error);
                    }
                }
            }
            if let Some(error) = state.error.clone() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
    });
}

/// Collects `(dotted path, options_path)` pairs from a schema tree.
pub fn collect_options_paths(
    schema: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) {
    let meta = crate::settings_ui::schema_form::UiMetadata::from_schema(schema);
    if let Some(options_path) = meta.options_path {
        out.push((prefix.to_string(), options_path));
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (name, child) in properties {
            let child_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            collect_options_paths(child, &child_prefix, out);
        }
    }
}

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}
