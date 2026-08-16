//! Engines page: unified management of local inference engines.
//!
//! One place for sidecar binaries (llama-server / whisper-server /
//! VOICEVOX), model files, catalog state, and the per-engine settings
//! (enable toggle plus schema-driven config forms). Cloud providers stay on
//! the Plugins page. The sidecar rows are rendered by the shared
//! [`artifact_card`] module (also used by the Voice page), and the set of
//! sidecars per engine comes from the plugin snapshot's requirements rather
//! than a hardcoded table.

use std::path::PathBuf;
use std::sync::Arc;

use crate::ai_bridge::AiBridge;
use crate::settings_ui::artifact_card;
use crate::settings_ui::components::{self, BadgeTone, status_badge};
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::input::{AsyncData, SettingsInputState};
use crate::settings_ui::provider_form;
use crate::settings_ui::schema_form::{SchemaFormOptions, schema_object_form};
use ene_plugin_host::{ArtifactSnapshot, PluginHealthState, PluginSettingsSnapshot};
use serde_json::Value;

/// Local engines shown on this page, in display order. Whether an engine
/// runs a catalog-managed sidecar is derived from its snapshot's `sidecars`
/// (manifest requirements plus the built-in table), so new sidecar plugins
/// appear automatically.
const ENGINES: &[&str] = &["llama-server", "voicevox", "whisper", "kokoro", "onnx"];

/// Plugins whose profiles are model definitions (weights on disk).
const MODEL_PROFILE_PLUGINS: &[&str] = &["llama-cpp", "llama-server", "local-llm"];

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

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
    artifact_card::poll_artifact_actions(input);

    render_catalog(ui, draft, ai, input);
    ui.add_space(8.0);

    let snapshots = input.plugin_snapshots.data.clone().unwrap_or_default();
    let artifacts = input.artifact_snapshot.data.clone().unwrap_or_default();
    components::section_card(ui, "engines-list", &fl("engines-list"), |ui| {
        // Fixed display order for the built-ins, then any snapshot-declared
        // engine (third-party plugin with sidecar requirements or model
        // profiles) so a new sidecar plugin is never dropped from the page.
        let mut names: Vec<String> = ENGINES.iter().map(|name| (*name).to_string()).collect();
        for snapshot in &snapshots {
            let declared = !snapshot.sidecars.is_empty()
                || snapshot.profiles.as_ref().is_some_and(|profiles| {
                    profiles
                        .as_object()
                        .is_some_and(|object| !object.is_empty())
                });
            if declared && !names.iter().any(|name| name == &snapshot.id) {
                names.push(snapshot.id.clone());
            }
        }
        for (index, name) in names.iter().enumerate() {
            render_engine(ui, draft, ai, input, name, &snapshots, &artifacts);
            if index + 1 < names.len() {
                ui.separator();
            }
        }
    });
}

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
    let mut root_dir = artifact_cfg
        .get("root_dir")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut keys: Vec<(String, String)> = artifact_cfg
        .get("catalog_keys")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(|key| {
                    Some((
                        key.get("key_id")?.as_str()?.to_string(),
                        key.get("public_key_hex")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
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
            ui.label(fl("engines-catalog-root"));
            changed |= ui
                .add(egui::TextEdit::singleline(&mut root_dir).desired_width(300.0))
                .changed();
        });
        ui.label(fl("engines-catalog-keys"));
        let mut removed_key: Option<usize> = None;
        for (index, (key_id, public_key)) in keys.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::TextEdit::singleline(key_id).desired_width(140.0))
                    .changed();
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(public_key)
                            .desired_width(280.0)
                            .hint_text(fl("engines-catalog-key-hint")),
                    )
                    .changed();
                if ui.small_button("✕").clicked() {
                    removed_key = Some(index);
                }
            });
        }
        if let Some(index) = removed_key {
            keys.remove(index);
            changed = true;
        }
        if ui.small_button(fl("engines-catalog-add-key")).clicked() {
            keys.push((String::new(), String::new()));
            changed = true;
        }
        if !keys.is_empty()
            && !keys.iter().all(|(id, key)| {
                !id.trim().is_empty()
                    && key.trim().len() == 64
                    && key.trim().chars().all(|c| c.is_ascii_hexdigit())
            })
        {
            ui.weak(fl("engines-catalog-key-invalid"));
        }
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
            object.insert(
                "root_dir".to_string(),
                if root_dir.trim().is_empty() {
                    Value::Null
                } else {
                    Value::String(root_dir.trim().to_string())
                },
            );
            object.insert(
                "catalog_keys".to_string(),
                Value::Array(
                    keys.into_iter()
                        .map(|(key_id, public_key_hex)| {
                            Value::Object(
                                [
                                    ("key_id".to_string(), Value::String(key_id)),
                                    ("public_key_hex".to_string(), Value::String(public_key_hex)),
                                ]
                                .into_iter()
                                .collect(),
                            )
                        })
                        .collect(),
                ),
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

    if let Some(snapshot) = snapshot
        && !snapshot.sidecars.is_empty()
    {
        for sidecar in &snapshot.sidecars {
            artifact_card::render_artifact_card(ui, ai, input, artifacts, sidecar);
        }
    }
    render_models(ui, draft, ai, input, artifacts, name);
    render_config_form(ui, draft, ai, input, snapshot);
}

fn render_models(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    artifacts: &[ArtifactSnapshot],
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
                // Catalog-managed models get the same explicit
                // install/update/cancel/rollback/uninstall card as sidecars.
                if let Some(artifact_id) = profile
                    .get("artifact_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                {
                    artifact_card::render_artifact_card(ui, ai, input, artifacts, artifact_id);
                }
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
                            ui.weak(format!(
                                "{} ({})",
                                path.display(),
                                artifact_card::format_size(size)
                            ));
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
                                ui.weak(format!("{} ({})", url, fl("engines-model-not-installed")));
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
    if config_value.is_null() {
        config_value = Value::Object(serde_json::Map::new());
    }
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
                provider_form::render_config_actions(
                    ui,
                    draft,
                    ai,
                    input,
                    Some(snapshot),
                    &snapshot.id,
                );
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

/// Dynamic options for `x-ene-ui.options_path` fields (shared helper with
/// the Voice/Plugins pages, including the explicit load/reload button).
fn schema_form_options(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    snapshot: &PluginSettingsSnapshot,
) -> std::collections::BTreeMap<String, Vec<(String, String)>> {
    provider_form::schema_form_options(ui, ai, input, snapshot, &snapshot.id)
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
