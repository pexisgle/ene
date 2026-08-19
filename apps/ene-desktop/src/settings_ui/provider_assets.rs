//! Generic provider asset list / install UI via core HTTP.

use std::collections::HashMap;
use std::sync::Arc;

use ene_api::{ProviderAssetInstallPhase, ProviderAssetView};
use tokio::sync::oneshot;

use crate::core_session::CoreSession;
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::provider_form::{ProviderInfo, provider_display_name};
use serde_json::{Value, json};

#[derive(Debug, Default)]
struct PluginAssetState {
    assets: Vec<ProviderAssetView>,
    list_receiver: Option<oneshot::Receiver<Result<Vec<ProviderAssetView>, String>>>,
    list_error: Option<String>,
    list_loaded: bool,
}

#[derive(Debug, Default)]
struct InstallState {
    plugin: String,
    asset_id: Option<String>,
    job_id: Option<String>,
    start_receiver: Option<oneshot::Receiver<Result<String, String>>>,
    status_receiver:
        Option<oneshot::Receiver<Result<ene_api::ProviderAssetInstallStatusResponse, String>>>,
    error: Option<String>,
    progress: (u64, Option<u64>),
    apply_task: Option<String>,
    active: bool,
}

impl InstallState {
    fn reset(&mut self) {
        self.job_id = None;
        self.start_receiver = None;
        self.status_receiver = None;
        self.asset_id = None;
        self.apply_task = None;
        self.active = false;
        self.progress = (0, None);
    }
}

#[derive(Debug, Default)]
pub struct ProviderAssetsUi {
    by_plugin: HashMap<String, PluginAssetState>,
    install: InstallState,
    pub completed_path: Option<(String, String, String, String)>,
    sidecar_release: HashMap<String, String>,
    sidecar_variant: HashMap<String, String>,
    refresh_receiver: Option<oneshot::Receiver<Result<(), String>>>,
}

impl ProviderAssetsUi {
    pub fn poll(&mut self, session: &Arc<CoreSession>) {
        let mut finished_plugins = Vec::new();
        for state in self.by_plugin.values_mut() {
            if let Some(receiver) = state.list_receiver.as_mut() {
                match receiver.try_recv() {
                    Ok(Ok(assets)) => {
                        state.assets = assets;
                        state.list_error = None;
                        state.list_receiver = None;
                        state.list_loaded = true;
                    }
                    Ok(Err(err)) => {
                        state.list_error = Some(err);
                        state.list_receiver = None;
                        state.list_loaded = true;
                    }
                    Err(oneshot::error::TryRecvError::Empty) => {}
                    Err(oneshot::error::TryRecvError::Closed) => {
                        state.list_error = Some("asset list cancelled".to_owned());
                        state.list_receiver = None;
                    }
                }
            }
        }
        if let Some(receiver) = self.install.start_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(job_id)) => {
                    self.install.job_id = Some(job_id);
                    self.install.start_receiver = None;
                }
                Ok(Err(err)) => {
                    self.install.error = Some(err);
                    self.install.reset();
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.install.error = Some("install cancelled".to_owned());
                    self.install.reset();
                }
            }
        }
        if self.install.active
            && self.install.job_id.is_some()
            && self.install.start_receiver.is_none()
            && self.install.status_receiver.is_none()
        {
            let plugin = self.install.plugin.clone();
            let job_id = self.install.job_id.clone().unwrap_or_default();
            self.install.status_receiver =
                Some(session.poll_provider_asset_install_status(plugin, job_id));
        }
        if let Some(receiver) = self.install.status_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(status)) => {
                    self.install.progress = (status.received, status.total);
                    self.install.status_receiver = None;
                    match status.phase {
                        Some(ProviderAssetInstallPhase::Done) => {
                            if let Some(path) = status.local_path {
                                let task = self
                                    .install
                                    .apply_task
                                    .clone()
                                    .unwrap_or_else(|| "chat".to_owned());
                                let plugin = self.install.plugin.clone();
                                let asset_id = self.install.asset_id.clone().unwrap_or_default();
                                self.completed_path = Some((plugin, asset_id, path, task));
                            }
                            finished_plugins.push(self.install.plugin.clone());
                            self.install.reset();
                        }
                        Some(ProviderAssetInstallPhase::Failed) => {
                            self.install.error = status.error.or(Some("install failed".to_owned()));
                            self.install.reset();
                        }
                        _ => {}
                    }
                }
                Ok(Err(err)) => {
                    self.install.error = Some(err);
                    self.install.reset();
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.install.error = Some("install cancelled".to_owned());
                    self.install.reset();
                }
            }
        }
        if let Some(receiver) = self.refresh_receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    self.refresh_receiver = None;
                    for plugin in ["provider.gguf", "provider.voicevox"] {
                        self.invalidate_list(plugin);
                    }
                }
                Ok(Err(err)) => {
                    self.refresh_receiver = None;
                    tracing::warn!(error = %err, "catalog refresh failed");
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.refresh_receiver = None;
                }
            }
        }
        for plugin in finished_plugins {
            self.invalidate_list(&plugin);
        }
    }

    fn state_mut(&mut self, plugin: &str) -> &mut PluginAssetState {
        self.by_plugin.entry(plugin.to_owned()).or_default()
    }

    fn state(&self, plugin: &str) -> Option<&PluginAssetState> {
        self.by_plugin.get(plugin)
    }

    fn invalidate_list(&mut self, plugin: &str) {
        if let Some(state) = self.by_plugin.get_mut(plugin) {
            state.assets.clear();
            state.list_error = None;
            state.list_receiver = None;
            state.list_loaded = false;
        }
    }

    pub fn ensure_list(&mut self, session: &Arc<CoreSession>, plugin: &str) {
        if plugin.is_empty() || !plugin.starts_with("provider.") {
            return;
        }
        let needs_fetch = {
            let state = self.state_mut(plugin);
            if state.list_receiver.is_some() || state.list_loaded {
                return;
            }
            true
        };
        if needs_fetch {
            let receiver = session.fetch_provider_assets(plugin.to_owned());
            self.state_mut(plugin).list_receiver = Some(receiver);
        }
    }

    pub fn install_busy(&self) -> bool {
        self.install.active
    }

    pub fn refresh_catalogs(&mut self, session: &Arc<CoreSession>) {
        if self.refresh_receiver.is_some() {
            return;
        }
        self.refresh_receiver = Some(session.refresh_provider_asset_catalogs());
    }

    pub fn start_install(
        &mut self,
        session: &Arc<CoreSession>,
        plugin: &str,
        asset_id: &str,
        release_tag: Option<String>,
        variant_id: Option<String>,
        task: &str,
    ) {
        if self.install_busy() {
            return;
        }
        self.install.error = None;
        self.completed_path = None;
        plugin.clone_into(&mut self.install.plugin);
        self.install.apply_task = Some(task.to_owned());
        self.install.asset_id = Some(asset_id.to_owned());
        self.install.active = true;
        self.install.progress = (0, None);
        self.install.start_receiver = Some(session.begin_provider_asset_install(
            plugin.to_owned(),
            asset_id.to_owned(),
            release_tag,
            variant_id,
        ));
    }

    pub fn start_install_legacy(
        &mut self,
        session: &Arc<CoreSession>,
        plugin: &str,
        asset_id: &str,
        version: Option<String>,
        task: &str,
    ) {
        self.start_install(session, plugin, asset_id, version, None, task);
    }

    fn assets_filtered<'a>(
        &'a self,
        plugin: &str,
        kind: &str,
        seam: Option<&str>,
    ) -> impl Iterator<Item = &'a ProviderAssetView> {
        self.state(plugin)
            .into_iter()
            .flat_map(|state| &state.assets)
            .filter(move |asset| {
                asset.kind == kind
                    && seam.is_none_or(|needle| asset.seams.iter().any(|row| row == needle))
            })
    }

    pub fn sidecar_ready(&self, plugin: &str) -> bool {
        let Some(asset_id) = sidecar_asset_id(plugin) else {
            return false;
        };
        self.assets_filtered(plugin, "sidecar", None)
            .any(|asset| asset.id == asset_id && asset.installed)
    }

    fn sidecar_key(plugin: &str, asset_id: &str) -> String {
        format!("{plugin}:{asset_id}")
    }

    fn selected_release(&self, plugin: &str, asset: &ProviderAssetView) -> String {
        let key = Self::sidecar_key(plugin, &asset.id);
        if let Some(value) = self.sidecar_release.get(&key) {
            return value.clone();
        }
        asset
            .versions
            .iter()
            .find(|row| row.recommended)
            .map(|row| {
                if row.release_tag.is_empty() {
                    row.version.clone()
                } else {
                    row.release_tag.clone()
                }
            })
            .or_else(|| {
                asset.versions.first().map(|row| {
                    if row.release_tag.is_empty() {
                        row.version.clone()
                    } else {
                        row.release_tag.clone()
                    }
                })
            })
            .unwrap_or_default()
    }

    fn selected_variant<'a>(
        &'a self,
        plugin: &str,
        asset: &'a ProviderAssetView,
        release_tag: &str,
    ) -> Option<&'a ene_api::ProviderAssetVersionView> {
        let key = Self::sidecar_key(plugin, &asset.id);
        if let Some(variant_id) = self.sidecar_variant.get(&key) {
            return asset.versions.iter().find(|row| {
                row.variant_id == *variant_id
                    && (row.release_tag == release_tag || row.version.starts_with(release_tag))
            });
        }
        asset
            .versions
            .iter()
            .filter(|row| row.release_tag == release_tag || row.version.starts_with(release_tag))
            .find(|row| row.recommended)
            .or_else(|| {
                asset.versions.iter().find(|row| {
                    row.release_tag == release_tag || row.version.starts_with(release_tag)
                })
            })
    }

    fn list_loading(&self, plugin: &str) -> bool {
        self.state(plugin)
            .is_some_and(|state| state.list_receiver.is_some())
    }

    fn list_loaded(&self, plugin: &str) -> bool {
        self.state(plugin).is_some_and(|state| state.list_loaded)
    }

    fn list_error(&self, plugin: &str) -> Option<&str> {
        self.state(plugin)
            .and_then(|state| state.list_error.as_deref())
    }
}

pub fn render_engines_assets(
    ui: &mut egui::Ui,
    assets: &mut ProviderAssetsUi,
    session: &Arc<CoreSession>,
    catalog: &[ProviderInfo],
) {
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "engines-assets-hint"
    ));
    let local_plugins: Vec<&ProviderInfo> = catalog.iter().filter(|plugin| plugin.local).collect();
    if local_plugins.is_empty() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "engines-assets-empty"
        ));
        return;
    }
    for plugin in local_plugins {
        ui.add_space(6.0);
        ui.strong(provider_display_name(&plugin.id));
        render_sidecar_section(ui, assets, session, &plugin.id);
    }
}

pub fn render_sidecar_hint(ui: &mut egui::Ui, assets: &ProviderAssetsUi, plugin: &str) {
    if assets.sidecar_ready(plugin) {
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
    if let Some(error) = assets.list_error(plugin) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let rows: Vec<ProviderAssetView> = assets
        .assets_filtered(plugin, "weight", Some(seam))
        .cloned()
        .collect();
    if rows.is_empty() {
        if assets.list_loading(plugin) {
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
        .map_or(selected_id.as_str(), |row| row.label.as_str());
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
        if row.installed
            && ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "ai-local-use"))
                .clicked()
            && let Some(path) = &row.local_path
        {
            apply_weight_binding(draft, task, plugin, &row.id, path);
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
            assets.start_install_legacy(session, plugin, &row.id, None, task);
        }
    });
    if assets.install_busy()
        && assets.install.plugin == plugin
        && assets.install.asset_id.as_deref() == Some(row.id.as_str())
    {
        render_install_progress(ui, assets.install.progress);
    }
    if assets.install.plugin == plugin
        && let Some(error) = &assets.install.error
    {
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
    if let Some(error) = assets.list_error(plugin) {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
        return;
    }
    let rows: Vec<ProviderAssetView> = assets
        .assets_filtered(plugin, "sidecar", None)
        .cloned()
        .collect();
    if rows.is_empty() {
        if assets.list_loading(plugin) {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-assets-loading"
            ));
        } else if assets.list_loaded(plugin) {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-assets-no-sidecars"
            ));
        }
        return;
    }
    ui.horizontal(|ui| {
        if ui
            .button(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "provider-assets-refresh-catalog"
            ))
            .clicked()
        {
            assets.refresh_catalogs(session);
        }
    });
    for row in rows {
        ui.separator();
        if !row.description.is_empty() {
            ui.label(&row.description);
        }
        ui.horizontal(|ui| {
            ui.strong(&row.label);
            if row.installed {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "provider-assets-installed"
                ));
            }
        });
        if row.installed {
            ui.weak(
                row.local_path
                    .clone()
                    .unwrap_or_else(|| row.active_version.clone().unwrap_or_default()),
            );
        } else {
            let mut release_tag = assets.selected_release(plugin, &row);
            let releases: Vec<String> = row
                .versions
                .iter()
                .map(|version| {
                    if version.release_tag.is_empty() {
                        version
                            .version
                            .split('/')
                            .next()
                            .unwrap_or(version.version.as_str())
                            .to_owned()
                    } else {
                        version.release_tag.clone()
                    }
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            if !releases.is_empty() {
                let key = ProviderAssetsUi::sidecar_key(plugin, &row.id);
                let mut selected_release = release_tag.clone();
                egui::ComboBox::from_id_salt(format!(
                    "provider-assets-release-{}-{}",
                    plugin, row.id
                ))
                .selected_text(&selected_release)
                .show_ui(ui, |ui| {
                    for tag in &releases {
                        if ui.selectable_label(selected_release == *tag, tag).clicked() {
                            selected_release.clone_from(tag);
                            assets
                                .sidecar_release
                                .insert(key.clone(), selected_release.clone());
                        }
                    }
                });
                release_tag = assets
                    .sidecar_release
                    .get(&key)
                    .cloned()
                    .unwrap_or(selected_release);
                assets
                    .sidecar_release
                    .entry(key.clone())
                    .or_insert_with(|| release_tag.clone());
            }
            let variants: Vec<&ene_api::ProviderAssetVersionView> = row
                .versions
                .iter()
                .filter(|version| {
                    version.release_tag == release_tag
                        || version.version.starts_with(&format!("{release_tag}/"))
                })
                .collect();
            let mut selected_variant = assets
                .selected_variant(plugin, &row, &release_tag)
                .map(|row| row.variant_id.clone())
                .unwrap_or_default();
            if !variants.is_empty() {
                egui::ComboBox::from_id_salt(format!(
                    "provider-assets-variant-{}-{}",
                    plugin, row.id
                ))
                .selected_text(
                    variants
                        .iter()
                        .find(|version| version.variant_id == selected_variant)
                        .map_or(selected_variant.as_str(), |version| {
                            if version.label.is_empty() {
                                version.variant_id.as_str()
                            } else {
                                version.label.as_str()
                            }
                        }),
                )
                .show_ui(ui, |ui| {
                    for version in &variants {
                        let label = if version.label.is_empty() {
                            version.variant_id.as_str()
                        } else {
                            version.label.as_str()
                        };
                        if ui
                            .selectable_label(selected_variant == version.variant_id, label)
                            .clicked()
                        {
                            selected_variant.clone_from(&version.variant_id);
                            assets.sidecar_variant.insert(
                                ProviderAssetsUi::sidecar_key(plugin, &row.id),
                                selected_variant.clone(),
                            );
                        }
                    }
                });
                assets
                    .sidecar_variant
                    .entry(ProviderAssetsUi::sidecar_key(plugin, &row.id))
                    .or_insert_with(|| selected_variant.clone());
            }
            let label = if assets.install_busy()
                && assets.install.plugin == plugin
                && assets.install.asset_id.as_deref() == Some(row.id.as_str())
            {
                i18n_embed_fl::fl!(crate::i18n::loader(), "provider-assets-downloading")
            } else {
                i18n_embed_fl::fl!(crate::i18n::loader(), "provider-assets-install")
            };
            if ui
                .add_enabled(!assets.install_busy(), egui::Button::new(label))
                .clicked()
            {
                let task = if plugin == "provider.voicevox" {
                    "tts"
                } else {
                    "sidecar"
                };
                assets.start_install(
                    session,
                    plugin,
                    &row.id,
                    Some(release_tag),
                    Some(selected_variant),
                    task,
                );
            }
        }
        if assets.install_busy()
            && assets.install.plugin == plugin
            && assets.install.asset_id.as_deref() == Some(row.id.as_str())
        {
            render_install_progress(ui, assets.install.progress);
        }
    }
    if assets.install.plugin == plugin
        && let Some(error) = &assets.install.error
    {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

fn sidecar_asset_id(plugin: &str) -> Option<&'static str> {
    match plugin {
        "provider.gguf" => Some("llama-server"),
        "provider.voicevox" => Some("voicevox-engine"),
        _ => None,
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
