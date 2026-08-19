//! Generic provider asset list / install UI via core HTTP.

use std::sync::Arc;

use ene_api::{ProviderAssetInstallPhase, ProviderAssetView};
use tokio::sync::oneshot;

use crate::core_session::CoreSession;
use crate::settings_ui::draft::SettingsDraft;
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct ProviderAssetsUi {
    plugin: String,
    assets: Vec<ProviderAssetView>,
    list_receiver: Option<oneshot::Receiver<Result<Vec<ProviderAssetView>, String>>>,
    list_error: Option<String>,
    install_asset_id: Option<String>,
    install_receiver:
        Option<oneshot::Receiver<Result<ene_api::ProviderAssetInstallStatusResponse, String>>>,
    install_error: Option<String>,
    install_progress: (u64, Option<u64>),
    pub completed_path: Option<(String, String, String)>,
    apply_task: Option<String>,
}

impl ProviderAssetsUi {
    pub fn poll(&mut self) {
        if let Some(receiver) = self.list_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(assets)) => {
                    self.assets = assets;
                    self.list_error = None;
                    self.list_receiver = None;
                }
                Ok(Err(err)) => {
                    self.list_error = Some(err);
                    self.list_receiver = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.list_error = Some("asset list cancelled".to_owned());
                    self.list_receiver = None;
                }
            }
        }
        if let Some(receiver) = self.install_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(status)) => {
                    self.install_progress = (status.received, status.total);
                    match status.phase {
                        Some(ProviderAssetInstallPhase::Done) => {
                            if let Some(path) = status.local_path {
                                let task =
                                    self.apply_task.clone().unwrap_or_else(|| "chat".to_owned());
                                let asset_id = self.install_asset_id.clone().unwrap_or_default();
                                self.completed_path = Some((asset_id, path, task));
                            }
                            self.install_receiver = None;
                            self.install_asset_id = None;
                        }
                        Some(ProviderAssetInstallPhase::Failed) => {
                            self.install_error = status.error.or(Some("install failed".to_owned()));
                            self.install_receiver = None;
                            self.install_asset_id = None;
                        }
                        _ => {}
                    }
                }
                Ok(Err(err)) => {
                    self.install_error = Some(err);
                    self.install_receiver = None;
                    self.install_asset_id = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.install_error = Some("install cancelled".to_owned());
                    self.install_receiver = None;
                    self.install_asset_id = None;
                }
            }
        }
    }

    pub fn ensure_list(&mut self, session: &Arc<CoreSession>, plugin: &str) {
        if plugin.is_empty() || !plugin.starts_with("provider.") {
            return;
        }
        if self.plugin != plugin {
            self.plugin = plugin.to_owned();
            self.assets.clear();
            self.list_receiver = None;
            self.list_error = None;
        }
        if self.list_receiver.is_some() || !self.assets.is_empty() || self.list_error.is_some() {
            return;
        }
        self.list_receiver = Some(session.fetch_provider_assets(plugin.to_owned()));
    }

    pub fn install_busy(&self) -> bool {
        self.install_receiver.is_some()
    }

    pub fn start_install(
        &mut self,
        session: &Arc<CoreSession>,
        plugin: &str,
        asset_id: &str,
        version: Option<String>,
        task: &str,
    ) {
        if self.install_busy() {
            return;
        }
        self.install_error = None;
        self.completed_path = None;
        self.apply_task = Some(task.to_owned());
        self.install_asset_id = Some(asset_id.to_owned());
        self.install_receiver =
            Some(session.install_provider_asset(plugin.to_owned(), asset_id.to_owned(), version));
    }

    pub fn assets_filtered<'a>(
        &'a self,
        kind: &str,
        seam: Option<&str>,
    ) -> impl Iterator<Item = &'a ProviderAssetView> {
        self.assets.iter().filter(move |asset| {
            asset.kind == kind
                && seam.is_none_or(|needle| asset.seams.iter().any(|row| row == needle))
        })
    }

    pub fn sidecar_ready(&self) -> bool {
        self.assets
            .iter()
            .any(|asset| asset.kind == "sidecar" && asset.id == "llama-server" && asset.installed)
    }
}

pub fn render_sidecar_hint(ui: &mut egui::Ui, assets: &ProviderAssetsUi) {
    if assets.sidecar_ready() {
        return;
    }
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "provider-assets-sidecar-missing"
    ));
}

pub fn render_weight_picker(
    ui: &mut egui::Ui,
    assets: &mut ProviderAssetsUi,
    session: &Arc<CoreSession>,
    draft: &mut SettingsDraft,
    plugin: &str,
    task: &str,
    seam: &str,
    selected_id: &mut String,
) {
    assets.ensure_list(session, plugin);
    if let Some(error) = &assets.list_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let rows: Vec<ProviderAssetView> = assets
        .assets_filtered("weight", Some(seam))
        .cloned()
        .collect();
    if rows.is_empty() {
        if assets.list_receiver.is_some() {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-assets-loading"
            ));
        }
        return;
    }
    if selected_id.is_empty() {
        if let Some(row) = rows.iter().find(|row| row.recommended) {
            selected_id.clone_from(&row.id);
        } else if let Some(row) = rows.first() {
            selected_id.clone_from(&row.id);
        }
    }
    let selected_label = rows
        .iter()
        .find(|row| row.id == *selected_id)
        .map(|row| row.label.as_str())
        .unwrap_or(selected_id.as_str());
    egui::ComboBox::from_id_salt(format!("provider-assets-weight-{task}"))
        .selected_text(selected_label)
        .show_ui(ui, |ui| {
            for row in &rows {
                if ui
                    .selectable_label(selected_id.as_str() == row.id, &row.label)
                    .clicked()
                {
                    selected_id.clone_from(&row.id);
                }
            }
        });
    let Some(row) = rows.iter().find(|row| row.id == *selected_id) else {
        return;
    };
    ui.horizontal(|ui| {
        if row.installed {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-use"))
                .clicked()
            {
                if let Some(path) = &row.local_path {
                    apply_weight_binding(draft, task, plugin, &row.id, path);
                }
            }
        } else if ui
            .add_enabled(
                !assets.install_busy(),
                egui::Button::new(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "provider-assets-install"
                )),
            )
            .clicked()
        {
            assets.start_install(session, plugin, &row.id, None, task);
        }
    });
    if assets.install_busy() && assets.install_asset_id.as_deref() == Some(row.id.as_str()) {
        render_install_progress(ui, assets.install_progress);
    }
    if let Some(error) = &assets.install_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

pub fn render_sidecar_section(
    ui: &mut egui::Ui,
    assets: &mut ProviderAssetsUi,
    session: &Arc<CoreSession>,
    plugin: &str,
) {
    assets.ensure_list(session, plugin);
    let rows: Vec<ProviderAssetView> = assets.assets_filtered("sidecar", None).cloned().collect();
    for row in rows {
        ui.separator();
        ui.strong(&row.label);
        if row.installed {
            ui.weak(
                row.local_path
                    .clone()
                    .unwrap_or_else(|| row.active_version.clone().unwrap_or_default()),
            );
        } else if ui
            .add_enabled(
                !assets.install_busy(),
                egui::Button::new(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "provider-assets-install"
                )),
            )
            .clicked()
        {
            let version = row
                .versions
                .iter()
                .find(|version| version.recommended)
                .map(|version| version.version.clone());
            assets.start_install(session, plugin, &row.id, version, "sidecar");
        }
    }
    if assets.install_busy() {
        render_install_progress(ui, assets.install_progress);
    }
}

fn render_install_progress(ui: &mut egui::Ui, (received, total): (u64, Option<u64>)) {
    let fraction = total
        .filter(|value| *value > 0)
        .map_or(0.0, |value| received as f32 / value as f32);
    ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
}

fn apply_weight_binding(
    draft: &mut SettingsDraft,
    task: &str,
    plugin: &str,
    model: &str,
    path: &str,
) {
    let mut ai = draft
        .editing()
        .section_value("ai")
        .unwrap_or_else(|| json!({ "tasks": {} }));
    if !ai.get("tasks").is_some_and(Value::is_object) {
        ai["tasks"] = json!({});
    }
    ai["tasks"][task] = json!({
        "plugin": plugin,
        "model": model,
        "model_path": path,
    });
    draft.set_section_value("ai", ai);
}
