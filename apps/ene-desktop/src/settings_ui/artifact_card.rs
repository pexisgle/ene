//! Shared artifact management card (Engines page and Voice page).
//!
//! One card shows the installed / catalog versions of a sidecar artifact,
//! offers download/update, cancel, rollback, and uninstall (with two-step
//! confirmation), and renders live progress (download bytes plus stage).
//! Action errors are retained per artifact id so a failed install is not
//! silently dropped.

use std::sync::Arc;

use crate::ai_bridge::AiBridge;
use crate::settings_ui::input::{AsyncData, SettingsInputState};
use ene_plugin_host::{ArtifactProgress, ArtifactSnapshot, InstallStage};

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

/// Renders the artifact card for `artifact_id` inside the current UI.
///
/// `artifacts` is the latest host-side artifact snapshot; the card shows
/// the installed and catalog generations (version + size) independently,
/// with explicit download/update, cancel, rollback (two-step), and
/// uninstall (two-step) actions.
pub fn render_artifact_card(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    artifacts: &[ArtifactSnapshot],
    artifact_id: &str,
) {
    poll_artifact_progress(ui, ai, input, artifact_id);
    let artifact = artifacts.iter().find(|a| a.artifact_id == artifact_id);
    let Some(artifact) = artifact else {
        ui.weak(fl("engines-not-installed"));
        return;
    };
    if let Some(error) = &artifact.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(error) = input.artifact_errors.get(artifact_id) {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let kind = artifact
        .installed
        .as_ref()
        .map(|installed| installed.kind.as_str())
        .or_else(|| {
            artifact
                .catalog
                .as_ref()
                .map(|catalog| catalog.kind.as_str())
        })
        .unwrap_or("sidecar");
    let kind_label = match kind {
        "model" => fl("engines-model-artifact"),
        _ => fl("engines-sidecar"),
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{kind_label}:"));
        match &artifact.installed {
            Some(installed) => {
                ui.label(format!(
                    "{} v{} ({})",
                    fl("engines-installed-version"),
                    installed.version,
                    format_size(installed.size)
                ));
            }
            None => {
                ui.weak(fl("engines-not-installed"));
            }
        }
        if let Some(catalog) = &artifact.catalog {
            ui.weak(format!(
                "{} v{} ({})",
                fl("engines-catalog-version"),
                catalog.version,
                format_size(catalog.size)
            ));
        }
    });

    let updating = input.artifact_installs.contains_key(artifact_id);
    let cancelling = input.artifact_cancels.contains_key(artifact_id);
    ui.horizontal_wrapped(|ui| {
        let installing = updating || cancelling;
        if cancelling {
            ui.weak(fl("engines-cancelling"));
        } else if artifact.update_available && !updating {
            let label = if artifact.installed.is_some() {
                fl("engines-update")
            } else {
                fl("engines-install")
            };
            let hint = if artifact.installed.is_some() {
                fl("engines-update-hint")
            } else {
                fl("engines-install-hint")
            };
            if ui.small_button(label).on_hover_text(hint).clicked() {
                input.artifact_errors.remove(artifact_id);
                input.artifact_installs.insert(
                    artifact_id.to_string(),
                    ai.install_artifact(artifact_id.to_string(), None),
                );
            }
        } else if updating {
            ui.weak(fl("engines-updating"));
            if ui.small_button(fl("engines-cancel")).clicked() {
                input.artifact_cancels.insert(
                    artifact_id.to_string(),
                    ai.cancel_artifact_install(artifact_id.to_string()),
                );
            }
        }

        let uninstalling = input.artifact_uninstalls.contains_key(artifact_id);
        let rolling = input.artifact_rollbacks.contains_key(artifact_id);
        if artifact.installed.is_some() && !rolling && !uninstalling && !installing {
            let rollback_arm = confirm_arm(input, artifact_id, "rollback");
            if ui
                .small_button(if rollback_arm {
                    fl("engines-rollback-confirm")
                } else {
                    fl("engines-rollback")
                })
                .on_hover_text(fl("engines-rollback-hint"))
                .clicked()
            {
                if rollback_arm {
                    input.artifact_errors.remove(artifact_id);
                    input.artifact_rollbacks.insert(
                        artifact_id.to_string(),
                        ai.rollback_artifact(artifact_id.to_string()),
                    );
                } else {
                    arm_confirm(input, artifact_id, "rollback");
                }
            }
            let uninstall_arm = confirm_arm(input, artifact_id, "uninstall");
            if ui
                .small_button(if uninstall_arm {
                    fl("engines-uninstall-confirm")
                } else {
                    fl("engines-uninstall")
                })
                .on_hover_text(fl("engines-uninstall-hint"))
                .clicked()
            {
                if uninstall_arm {
                    input.artifact_errors.remove(artifact_id);
                    input.artifact_uninstalls.insert(
                        artifact_id.to_string(),
                        ai.uninstall_artifact(artifact_id.to_string()),
                    );
                } else {
                    arm_confirm(input, artifact_id, "uninstall");
                }
            }
        } else if rolling {
            ui.weak(fl("engines-rolling-back"));
        } else if uninstalling {
            ui.weak(fl("engines-uninstalling"));
        }
    });
}

/// Whether a two-step confirmation is armed for `artifact_id|action`.
fn confirm_arm(input: &SettingsInputState, artifact_id: &str, action: &str) -> bool {
    input
        .artifact_arm
        .get(&format!("{artifact_id}|{action}"))
        .copied()
        .unwrap_or(false)
}

/// Arms a two-step confirmation, disarming every other action for the
/// artifact so only one button is in the confirm state at a time.
fn arm_confirm(input: &mut SettingsInputState, artifact_id: &str, action: &str) {
    let keys: Vec<String> = input
        .artifact_arm
        .keys()
        .filter(|key| key.starts_with(&format!("{artifact_id}|")))
        .cloned()
        .collect();
    for key in keys {
        input.artifact_arm.insert(key, false);
    }
    input
        .artifact_arm
        .insert(format!("{artifact_id}|{action}"), true);
}

/// Polls in-flight artifact installs/rollbacks/uninstalls/cancels, retains
/// per-artifact errors, and refreshes the artifact snapshot on completion.
pub fn poll_artifact_actions(input: &mut SettingsInputState) {
    poll_action_map(
        &mut input.artifact_installs,
        &mut input.artifact_errors,
        &mut input.artifact_snapshot,
    );
    poll_action_map(
        &mut input.artifact_rollbacks,
        &mut input.artifact_errors,
        &mut input.artifact_snapshot,
    );
    poll_action_map(
        &mut input.artifact_uninstalls,
        &mut input.artifact_errors,
        &mut input.artifact_snapshot,
    );
    let mut finished_cancels: Vec<String> = Vec::new();
    for (id, receiver) in &mut input.artifact_cancels {
        match receiver.try_recv() {
            Ok(Ok(()) | Err(_)) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                finished_cancels.push(id.clone());
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
    }
    for id in finished_cancels {
        input.artifact_cancels.remove(&id);
    }
    // Confirmations stay armed until the user clicks the confirm button
    // (or arms a different action for the same artifact); they are cleared
    // when the action actually starts.
    let started: Vec<String> = input
        .artifact_arm
        .iter()
        .filter(|(_, armed)| **armed)
        .map(|(id, _)| id.clone())
        .filter(|id| {
            id.split('|')
                .next()
                .is_some_and(|id| live_actions(input, id))
        })
        .collect();
    for id in started {
        input.artifact_arm.insert(id, false);
    }
    input.artifact_progress.poll();
}

/// Whether any artifact action is in flight for `artifact_id`.
fn live_actions(input: &SettingsInputState, artifact_id: &str) -> bool {
    input.artifact_installs.contains_key(artifact_id)
        || input.artifact_rollbacks.contains_key(artifact_id)
        || input.artifact_uninstalls.contains_key(artifact_id)
}

fn poll_action_map<R>(
    actions: &mut std::collections::HashMap<
        String,
        tokio::sync::oneshot::Receiver<Result<R, String>>,
    >,
    errors: &mut std::collections::BTreeMap<String, String>,
    snapshot: &mut AsyncData<Vec<ene_plugin_host::ArtifactSnapshot>>,
) {
    let mut finished: Vec<(String, Option<String>)> = Vec::new();
    for (id, receiver) in actions.iter_mut() {
        match receiver.try_recv() {
            Ok(Ok(_)) => finished.push((id.clone(), None)),
            Ok(Err(e)) => finished.push((id.clone(), Some(e))),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                finished.push((id.clone(), Some("operation cancelled".to_string())));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
    }
    for (id, error) in finished {
        actions.remove(&id);
        if let Some(error) = error {
            errors.insert(id.clone(), error);
        }
        // The snapshot is stale after any mutation; refetch on the next
        // frame.
        *snapshot = AsyncData::default();
    }
}

/// Polls the runtime's in-flight artifact progress and renders a progress
/// bar (bytes + stage) while an install is running.
///
/// The progress map is refetched every frame while any install is in
/// flight, so a long download keeps updating without a restart.
fn poll_artifact_progress(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    artifact_id: &str,
) {
    let in_flight = input.artifact_installs.contains_key(artifact_id);
    input.artifact_progress.poll();
    if in_flight && !input.artifact_progress.loading() {
        // Keep the fetched snapshot for display while the next fetch is in
        // flight, so the progress bar never flickers through empty data.
        input
            .artifact_progress
            .refresh(ai.fetch_artifact_progress());
    }
    let Some(progress) = input
        .artifact_progress
        .data
        .as_ref()
        .and_then(|map| map.get(artifact_id))
        .copied()
        .flatten()
    else {
        return;
    };
    render_progress_bar(ui, progress);
}

/// Renders one progress snapshot as a labeled progress bar.
fn render_progress_bar(ui: &mut egui::Ui, progress: ArtifactProgress) {
    let stage_label = match progress.stage {
        InstallStage::Download => fl("engines-stage-download"),
        InstallStage::Verify => fl("engines-stage-verify"),
        InstallStage::Extract => fl("engines-stage-extract"),
        InstallStage::Activate => fl("engines-stage-activate"),
    };
    let total = progress.total_bytes.unwrap_or(0);
    if total == 0 {
        ui.weak(format!("{}…", fl("engines-downloading")));
        return;
    }
    let fraction = (progress.downloaded_bytes as f32 / total as f32).clamp(0.0, 1.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::ProgressBar::new(fraction)
                .desired_width(ui.available_width().min(180.0))
                .text(format!(
                    "{stage_label} · {} / {}",
                    format_size(progress.downloaded_bytes),
                    format_size(total)
                )),
        );
    });
}

/// Human-readable byte size.
pub fn format_size(bytes: u64) -> String {
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
