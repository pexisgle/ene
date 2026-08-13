//! Engines page: unified management of local inference engines.
//!
//! One place for sidecar binaries (llama-server / whisper-server), model
//! files, catalog state, and the per-engine settings (enable toggle plus
//! schema-driven config forms). Cloud providers stay on the Plugins page.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ai_bridge::AiBridge;
use crate::settings_ui::components::{self, BadgeTone, status_badge};
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::input::{AsyncData, SettingsInputState};
use crate::settings_ui::schema_form::{SchemaFormOptions, schema_object_form};
use ene_plugin_host::{ArtifactSnapshot, PluginHealthState, PluginSettingsSnapshot};
use serde_json::Value;

/// Local engines shown on this page: plugin name + whether it runs a
/// sidecar binary that the artifact catalog manages.
const ENGINES: &[(&str, bool)] = &[
    ("llama-server", true),
    ("voicevox", false),
    ("whisper", true),
    ("kokoro", false),
    ("onnx", false),
];

/// Plugins whose profiles are model definitions (weights on disk).
const MODEL_PROFILE_PLUGINS: &[&str] = &["llama-server", "local-llm"];

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

fn sidecar_artifact_id(plugin: &str) -> &'static str {
    if plugin == "whisper" {
        "whisper-server"
    } else {
        "llama-server"
    }
}

/// Renders the Engines page.
pub fn render(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    if !input.plugin_snapshots.started() {
        input.plugin_snapshots.start(ai.fetch_plugin_snapshots());
    }
    input.plugin_snapshots.poll();
    if !input.artifact_snapshot.started() {
        input.artifact_snapshot.start(ai.fetch_artifact_snapshot());
    }
    input.artifact_snapshot.poll();
    poll_artifact_actions(input);

    render_catalog(ui, draft, ai, input);
    ui.add_space(8.0);

    let snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();
    let artifacts = input.artifact_snapshot.data.clone().unwrap_or_default();
    components::section_card(ui, "engines-list", &fl("engines-list"), |ui| {
        for (index, (name, has_sidecar)) in ENGINES.iter().enumerate() {
            render_engine(
                ui,
                draft,
                ai,
                input,
                name,
                *has_sidecar,
                &snapshots,
                &artifacts,
            );
            if index + 1 < ENGINES.len() {
                ui.separator();
            }
        }
    });
}

/// Catalog configuration and refresh controls.
fn render_catalog(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
) {
    let path = "plugins.artifact";
    let mut artifact_cfg = draft
        .editing()
        .get_path(path)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let mut enabled = artifact_cfg
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut url = artifact_cfg
        .get("catalog_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut changed = false;

    components::section_card(ui, "engines-catalog", &fl("engines-catalog"), |ui| {
        changed |= ui
            .checkbox(&mut enabled, fl("engines-catalog-enable"))
            .changed();
        ui.horizontal(|ui| {
            ui.label(fl("engines-catalog-url"));
            changed |= ui
                .add(egui::TextEdit::singleline(&mut url).desired_width(300.0))
                .changed();
        });
        ui.horizontal(|ui| {
            let refreshing = input.catalog_refresh.is_some();
            if ui
                .add_enabled(!refreshing, egui::Button::new(fl("engines-refresh")))
                .on_hover_text(fl("engines-refresh-hint"))
                .clicked()
            {
                input.catalog_refresh = Some(ai.refresh_catalog());
            }
            if refreshing {
                ui.weak(fl("engines-catalog-refreshing"));
            }
            if let Some(receiver) = input.catalog_refresh.as_mut() {
                match receiver.try_recv() {
                    Ok(Ok(version)) => {
                        input.catalog_refresh = None;
                        input.artifact_snapshot = AsyncData::default();
                        ui.weak(format!("{}: {version}", fl("engines-catalog-version")));
                    }
                    Ok(Err(e)) => {
                        input.catalog_refresh = None;
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                    Err(_) => input.catalog_refresh = None,
                }
            }
        });
        if !enabled || url.trim().is_empty() {
            ui.weak(fl("engines-catalog-disabled"));
        }
    });
    if changed {
        if let Some(object) = artifact_cfg.as_object_mut() {
            object.insert("enabled".to_string(), Value::Bool(enabled));
            object.insert(
                "catalog_url".to_string(),
                Value::String(url.trim().to_string()),
            );
        }
        draft.set_path(path, artifact_cfg);
    }
}

fn render_engine(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    name: &str,
    has_sidecar: bool,
    snapshots: &[PluginSettingsSnapshot],
    artifacts: &[ArtifactSnapshot],
) {
    let snapshot = snapshots.iter().find(|snapshot| snapshot.id == name);
    ui.horizontal(|ui| {
        ui.strong(name);
        let enabled_key = format!("plugins.list.{name}.enable");
        let mut enabled = draft
            .editing()
            .get_path(&enabled_key)
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| snapshot.is_some_and(|snapshot| snapshot.enabled));
        if ui.checkbox(&mut enabled, fl("plugins-enabled")).changed() {
            draft.set_path(&enabled_key, Value::Bool(enabled));
        }
        if let Some(snapshot) = snapshot {
            render_health_badge(ui, snapshot);
        } else {
            status_badge(ui, &fl("plugins-health-stopped"), BadgeTone::Neutral);
        }
    });

    if has_sidecar {
        render_sidecar_row(ui, ai, input, sidecar_artifact_id(name), artifacts);
    }
    render_models(ui, draft, input, name);
    render_config_form(ui, draft, ai, input, snapshot);
}

/// Sidecar binary row: installed version, update, and rollback.
fn render_sidecar_row(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    artifact_id: &str,
    artifacts: &[ArtifactSnapshot],
) {
    ui.horizontal(|ui| {
        ui.label(format!("{}:", fl("engines-sidecar")));
        let artifact = artifacts.iter().find(|a| a.artifact_id == artifact_id);
        match artifact {
            Some(artifact) => {
                match &artifact.installed {
                    Some(installed) => {
                        ui.label(format!(
                            "{} v{} ({}",
                            fl("engines-installed-version"),
                            installed.version,
                            format_size(installed.size)
                        ));
                        ui.label(")");
                    }
                    None => {
                        ui.weak(fl("engines-not-installed"));
                    }
                }
                if let Some(error) = &artifact.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                let updating = input.artifact_installs.contains_key(artifact_id);
                if artifact.update_available && !updating {
                    if ui
                        .small_button(fl("engines-update"))
                        .on_hover_text(fl("engines-update-hint"))
                        .clicked()
                    {
                        input.artifact_installs.insert(
                            artifact_id.to_string(),
                            ai.install_artifact(artifact_id.to_string(), None),
                        );
                    }
                } else if updating {
                    ui.weak(fl("engines-updating"));
                }
                let rolling = input.artifact_rollbacks.contains_key(artifact_id);
                if artifact.installed.is_some() && !rolling {
                    if ui
                        .small_button(fl("engines-rollback"))
                        .on_hover_text(fl("engines-rollback-hint"))
                        .clicked()
                    {
                        input.artifact_rollbacks.insert(
                            artifact_id.to_string(),
                            ai.rollback_artifact(artifact_id.to_string()),
                        );
                    }
                } else if rolling {
                    ui.weak(fl("engines-rolling-back"));
                }
            }
            None => {
                ui.weak(fl("engines-not-installed"));
            }
        }
    });
}

/// Model files section: profile list with paths, sizes, and removal.
fn render_models(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    input: &mut SettingsInputState,
    plugin: &str,
) {
    if !MODEL_PROFILE_PLUGINS.contains(&plugin) {
        return;
    }
    let profiles_path = format!("plugins.list.{plugin}.profiles");
    let Some(profiles) = draft.editing().get_path(&profiles_path) else {
        return;
    };
    let Some(object) = profiles.as_object() else {
        return;
    };
    if object.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(format!("{} ({})", fl("engines-models"), object.len()))
        .id_salt(("engines_models", plugin))
        .default_open(true)
        .show(ui, |ui| {
            let mut removed: Option<String> = None;
            let mut updated = None;
            for (name, profile) in object {
                ui.horizontal(|ui| {
                    ui.strong(name);
                    let path = profile
                        .get("model_path")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from);
                    let url = profile
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty());
                    let size = path
                        .as_ref()
                        .and_then(|path| std::fs::metadata(path).ok())
                        .map(|metadata| metadata.len());
                    match (path, size) {
                        (Some(path), Some(size)) => {
                            ui.weak(format!("{} ({})", path.display(), format_size(size)));
                        }
                        (Some(path), None) => {
                            ui.weak(format!(
                                "{} ({})",
                                path.display(),
                                fl("engines-model-missing")
                            ));
                        }
                        (None, _) => {
                            if let Some(url) = url {
                                ui.weak(format!(
                                    "{} ({})",
                                    url,
                                    fl("engines-model-download-on-use")
                                ));
                            }
                        }
                    }
                    let arm_key = format!("{plugin}|{name}");
                    let armed = input
                        .model_delete_arm
                        .get(&arm_key)
                        .copied()
                        .unwrap_or(false);
                    let delete_label = if armed {
                        fl("engines-model-delete-confirm")
                    } else {
                        fl("engines-model-delete")
                    };
                    if ui
                        .small_button(&delete_label)
                        .on_hover_text(fl("engines-model-delete-hint"))
                        .clicked()
                    {
                        if armed {
                            removed = Some(name.clone());
                        } else {
                            input.model_delete_arm.insert(arm_key.clone(), true);
                        }
                    } else if input
                        .model_delete_arm
                        .get(&arm_key)
                        .copied()
                        .unwrap_or(false)
                    {
                        input.model_delete_arm.insert(arm_key, false);
                    }
                });
            }
            if let Some(name) = removed {
                input.model_delete_arm.remove(&format!("{plugin}|{name}"));
                let mut profiles = profiles.clone();
                if let Some(object) = profiles.as_object_mut()
                    && let Some(profile) = object.remove(&name)
                    && let Some(path) = profile
                        .get("model_path")
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                    // Remove the cached file too, but only inside the
                    // managed models directory.
                    && let Ok(canonical) = path.canonicalize()
                    && let Ok(models) = ene_config::models_dir().canonicalize()
                    && canonical.starts_with(models)
                {
                    drop(std::fs::remove_file(canonical));
                }
                updated = Some(profiles);
            }
            if let Some(profiles) = updated {
                draft.set_path(&profiles_path, profiles);
            }
        });
}

/// Schema-driven config form for the engine's plugin config.
fn render_config_form(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: Option<&PluginSettingsSnapshot>,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    let schema_path = format!("plugins.list.{}.config", snapshot.id);
    let mut config_value = draft
        .editing()
        .get_path(&schema_path)
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if let Some(schema) = &snapshot.schema {
        egui::CollapsingHeader::new(fl("plugins-config"))
            .id_salt(("engines_config", snapshot.id.as_str()))
            .show(ui, |ui| {
                let options = schema_form_options(ui, ai, input, snapshot);
                if schema_object_form(
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
                ) {
                    draft.set_path(&schema_path, config_value);
                }
            });
    } else if !config_value
        .as_object()
        .is_none_or(serde_json::Map::is_empty)
    {
        egui::CollapsingHeader::new(fl("plugins-config"))
            .id_salt(("engines_config_raw", snapshot.id.as_str()))
            .show(ui, |ui| {
                if super::schema_form::raw_json_form(
                    ui,
                    &mut config_value,
                    &schema_path,
                    &Value::Null,
                    draft.applied_revision(),
                ) {
                    draft.set_path(&schema_path, config_value);
                }
            });
    }
}

/// Dynamic options for `x-ene-ui.options_path` fields, fetched once per
/// plugin and polled every frame (same contract as the Plugins page).
fn schema_form_options(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
) -> std::collections::BTreeMap<String, Vec<(String, String)>> {
    let Some(schema) = &snapshot.schema else {
        return std::collections::BTreeMap::new();
    };
    let state = input.plugin_options.entry(snapshot.id.clone()).or_default();
    state.poll();
    let mut fields = Vec::new();
    collect_options_paths(schema, "", &mut fields);
    if !state.started() {
        let plugin = snapshot.id.clone();
        state.start(ai.fetch_plugin_options(plugin, fields));
    }
    if state.loading() {
        ui.weak(fl("plugins-loading"));
    }
    if let Some(error) = state.error.clone() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(map) = &state.data
        && !map.is_empty()
    {
        return map.clone();
    }
    std::collections::BTreeMap::new()
}

fn collect_options_paths(
    schema: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, String)>,
) {
    let meta = super::schema_form::UiMetadata::from_schema(schema);
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

fn render_health_badge(ui: &mut egui::Ui, snapshot: &PluginSettingsSnapshot) {
    let (label, tone) = match snapshot.health {
        PluginHealthState::Running => (fl("plugins-health-running"), BadgeTone::Ok),
        PluginHealthState::Disabled => (fl("plugins-health-disabled"), BadgeTone::Error),
        PluginHealthState::RequirementsUnmet => {
            (fl("plugins-health-requirements_unmet"), BadgeTone::Warn)
        }
        PluginHealthState::Stopped => (fl("plugins-health-stopped"), BadgeTone::Neutral),
    };
    status_badge(ui, &label, tone);
}

/// Polls in-flight artifact installs/rollbacks; a completed action resets
/// the artifact snapshot so the next frame refetches fresh state.
fn poll_artifact_actions(input: &mut SettingsInputState) {
    let mut finished_installs: Vec<String> = Vec::new();
    for (id, receiver) in &mut input.artifact_installs {
        match receiver.try_recv() {
            Ok(Ok(_) | Err(_)) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                finished_installs.push(id.clone());
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
    }
    for id in finished_installs {
        input.artifact_installs.remove(&id);
        input.artifact_snapshot = AsyncData::default();
    }
    let mut finished_rollbacks: Vec<String> = Vec::new();
    for (id, receiver) in &mut input.artifact_rollbacks {
        match receiver.try_recv() {
            Ok(Ok(_) | Err(_)) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                finished_rollbacks.push(id.clone());
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
    }
    for id in finished_rollbacks {
        input.artifact_rollbacks.remove(&id);
        input.artifact_snapshot = AsyncData::default();
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
