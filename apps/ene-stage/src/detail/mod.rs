//! Detail window: eight new-core IA sections plus a session log.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use ene_api::{
    ApiClient, CharacterView, JobView, MemoryView, OccupantView, PluginView, ProviderAssetView,
    ScheduleView, SoulView,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::runtime::Handle;

use crate::i18n;
use crate::settings::DesktopSettings;
use crate::tasks::AsyncOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Home,
    Companion,
    Conversation,
    Voice,
    Memory,
    Work,
    Connections,
    System,
    Log,
}

impl DetailTab {
    pub const ALL: [DetailTab; 9] = [
        DetailTab::Home,
        DetailTab::Companion,
        DetailTab::Conversation,
        DetailTab::Voice,
        DetailTab::Memory,
        DetailTab::Work,
        DetailTab::Connections,
        DetailTab::System,
        DetailTab::Log,
    ];

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Home => i18n::fl("detail-tab-home"),
            Self::Companion => i18n::fl("detail-tab-companion"),
            Self::Conversation => i18n::fl("detail-tab-conversation"),
            Self::Voice => i18n::fl("detail-tab-voice"),
            Self::Memory => i18n::fl("detail-tab-memory"),
            Self::Work => i18n::fl("detail-tab-work"),
            Self::Connections => i18n::fl("detail-tab-connections"),
            Self::System => i18n::fl("detail-tab-system"),
            Self::Log => i18n::fl("detail-tab-log"),
        }
    }

    #[must_use]
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Home => &["home", "status", "health", "ready", "chat"],
            Self::Companion => &[
                "character",
                "export",
                "avatar",
                "body",
                "occupant",
                "alicia",
                "persona",
            ],
            Self::Conversation => &[
                "chat",
                "model",
                "api",
                "key",
                "base_url",
                "openai",
                "provider",
                "credentials",
            ],
            Self::Voice => &["tts", "stt", "mic", "caption", "spotlight", "voice"],
            Self::Memory => &["recall", "pending", "memory"],
            Self::Work => &[
                "job", "fork", "compact", "export", "session", "schedule", "work",
            ],
            Self::Connections => &["plugin", "mcp", "fiber", "profile", "asset"],
            Self::System => &[
                "backup",
                "restore",
                "reload",
                "json",
                "settings",
                "schema",
                "click-through",
                "data",
            ],
            Self::Log => &["thinking", "tool", "session", "log"],
        }
    }

    #[must_use]
    pub fn matches_search(self, query: &str) -> bool {
        self.search_rank(query).is_some()
    }

    /// Lower is a better match. `None` means the tab should be hidden.
    #[must_use]
    pub fn search_rank(self, query: &str) -> Option<u8> {
        if query.is_empty() {
            return Some(0);
        }
        let q = query.to_ascii_lowercase();
        let label = self.label().to_ascii_lowercase();
        if label.starts_with(&q) {
            return Some(1);
        }
        if label.contains(&q) {
            return Some(2);
        }
        if self
            .keywords()
            .iter()
            .any(|word| *word == q || word.starts_with(&q))
        {
            return Some(3);
        }
        if self
            .keywords()
            .iter()
            .any(|word| word.contains(&q) || q.contains(*word))
        {
            return Some(4);
        }
        None
    }
}

fn best_search_tab(query: &str) -> Option<DetailTab> {
    DetailTab::ALL
        .into_iter()
        .filter_map(|tab| tab.search_rank(query).map(|rank| (rank, tab)))
        .min_by_key(|(rank, tab)| {
            (
                *rank,
                DetailTab::ALL
                    .iter()
                    .position(|item| *item == *tab)
                    .unwrap_or(DetailTab::ALL.len()),
            )
        })
        .map(|(_, tab)| tab)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Thinking,
    Tool,
    Session,
    Job,
    Affect,
    Inner,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub kind: LogKind,
    pub text: String,
}

#[derive(Default)]
pub struct DetailUiState {
    pub visible: bool,
    pub tab: DetailTab,
    pub search: String,
    pub log: Vec<LogEntry>,
    pub core_settings_text: String,
    pub core_patch_text: String,
    pub chat_plugin: String,
    pub chat_model: String,
    pub chat_base_url: String,
    pub chat_api_key: String,
    pub ai_chat_key_set: bool,
    pub providers: Value,
    pub classifier_plugin: String,
    pub embedding_plugin: String,
    pub proactive_plugin: String,
    pub tts_plugin: String,
    pub stt_plugin: String,
    pub plugins_profile: String,
    pub core_status: String,
    pub connections_status: String,
    pub health: String,
    pub unconfigured: Vec<String>,
    pub memories: Vec<MemoryView>,
    pub pending_memories: Vec<MemoryView>,
    pub soul: Option<SoulView>,
    pub characters: Vec<CharacterView>,
    pub occupants: Vec<OccupantView>,
    pub body_ref_draft: String,
    pub jobs: Vec<JobView>,
    pub schedules: Vec<ScheduleView>,
    pub plugins: Vec<PluginView>,
    pub provider_assets_plugin: String,
    pub provider_assets: Vec<ProviderAssetView>,
    pub provider_install_jobs: HashMap<String, String>,
    pub provider_models: Vec<String>,
    pub provider_model_filter: String,
    pub mcp_json: String,
    pub schema_json: String,
    pub usage_text: String,
    pub spans_text: String,
    pub save_local_pending: bool,
    pub restore_id: String,
    pub restore_confirm: bool,
    pub session_id: String,
    pub open_spotlight: bool,
    loaded: DetailLoaded,
}

#[derive(Default)]
struct DetailLoaded {
    settings: bool,
    memory: bool,
    character: bool,
    jobs: bool,
    plugins: bool,
    provider_assets: bool,
    health: bool,
}

impl DetailUiState {
    pub fn push_log(&mut self, kind: LogKind, text: String) {
        if text.is_empty() {
            return;
        }
        self.log.push(LogEntry { kind, text });
        if self.log.len() > 500 {
            let drain = self.log.len() - 500;
            self.log.drain(0..drain);
        }
    }

    pub fn invalidate_settings(&mut self) {
        self.loaded.settings = false;
    }

    /// Explicit navigation wins over the search box; otherwise search re-selects the tab every frame.
    pub fn select_tab(&mut self, tab: DetailTab) {
        self.tab = tab;
        self.search.clear();
    }
}

pub fn parse_core_fields(json: &str, state: &mut DetailUiState) {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return;
    };
    let effective = value.get("effective").unwrap_or(&value);
    state.chat_plugin = nested_string(effective, &["ai", "tasks", "chat", "plugin"]);
    state.chat_model = nested_string(effective, &["ai", "tasks", "chat", "model"]);
    state.chat_base_url = nested_string(effective, &["ai", "tasks", "chat", "base_url"]);
    state.ai_chat_key_set = effective
        .get("ai_chat_key_set")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    state.providers = effective
        .get("providers")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    state.classifier_plugin = nested_string(effective, &["ai", "tasks", "classifier", "plugin"]);
    state.embedding_plugin = nested_string(effective, &["ai", "tasks", "embedding", "plugin"]);
    state.proactive_plugin = nested_string(effective, &["ai", "tasks", "proactive", "plugin"]);
    state.tts_plugin = nested_string(effective, &["ai", "tasks", "tts", "plugin"]);
    state.stt_plugin = nested_string(effective, &["ai", "tasks", "stt", "plugin"]);
    state.plugins_profile = nested_string(effective, &["plugins", "profile"]);
    state.unconfigured.clear();
    for (name, path) in [
        ("chat", ["ai", "tasks", "chat", "plugin"]),
        ("classifier", ["ai", "tasks", "classifier", "plugin"]),
        ("embedding", ["ai", "tasks", "embedding", "plugin"]),
        ("proactive", ["ai", "tasks", "proactive", "plugin"]),
        ("tts", ["ai", "tasks", "tts", "plugin"]),
        ("stt", ["ai", "tasks", "stt", "plugin"]),
    ] {
        let plugin = nested_string(effective, &path);
        let missing_plugin = plugin.is_empty() || plugin == "echo";
        let missing_chat_model = name == "chat"
            && nested_string(effective, &["ai", "tasks", "chat", "model"]).is_empty();
        let missing_chat_key =
            name == "chat" && plugin_needs_key(&plugin, &state.providers) && !state.ai_chat_key_set;
        if missing_plugin || missing_chat_model || missing_chat_key {
            state.unconfigured.push(name.to_owned());
        }
    }
}

fn nested_string(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        current = next;
    }
    current.as_str().unwrap_or("").to_owned()
}

#[must_use]
pub fn is_provider_plugin_id(id: &str) -> bool {
    id.starts_with("provider.") && id.len() > "provider.".len()
}

#[must_use]
pub fn default_provider_assets_plugin(chat_plugin: &str, plugins: &[PluginView]) -> String {
    if is_provider_plugin_id(chat_plugin) {
        return chat_plugin.to_owned();
    }
    plugins
        .iter()
        .find(|plugin| is_provider_plugin_id(&plugin.plugin))
        .map(|plugin| plugin.plugin.clone())
        .unwrap_or_default()
}

#[must_use]
pub fn plugin_needs_key(plugin: &str, providers: &Value) -> bool {
    providers
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(plugin))
        .and_then(|row| row.get("needs_key").and_then(Value::as_bool))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSetupGap {
    Plugin,
    Model,
    ApiKey,
}

#[must_use]
pub fn chat_setup_gap(state: &DetailUiState) -> Option<ChatSetupGap> {
    if state.chat_plugin.is_empty() || state.chat_plugin == "echo" {
        return Some(ChatSetupGap::Plugin);
    }
    if state.chat_model.is_empty() {
        return Some(ChatSetupGap::Model);
    }
    if plugin_needs_key(&state.chat_plugin, &state.providers) && !state.ai_chat_key_set {
        return Some(ChatSetupGap::ApiKey);
    }
    None
}

#[must_use]
pub fn chat_setup_status(gap: ChatSetupGap) -> String {
    match gap {
        ChatSetupGap::Plugin | ChatSetupGap::Model => i18n::fl("chat-unconfigured"),
        ChatSetupGap::ApiKey => i18n::fl("chat-missing-key"),
    }
}

#[must_use]
pub fn home_chat_next_step(state: &DetailUiState) -> String {
    if chat_setup_gap(state) == Some(ChatSetupGap::ApiKey) {
        i18n::fl("home-next-chat-key")
    } else {
        i18n::fl("home-next-chat")
    }
}

#[must_use]
pub fn blocking_unconfigured(tasks: &[String]) -> Vec<&str> {
    tasks
        .iter()
        .map(String::as_str)
        .filter(|task| *task == "chat")
        .collect()
}

#[must_use]
pub fn optional_unconfigured(tasks: &[String]) -> Vec<&str> {
    tasks
        .iter()
        .map(String::as_str)
        .filter(|task| *task != "chat")
        .collect()
}

#[must_use]
pub fn list_models_status(models: &[String], error: Option<&str>) -> String {
    if let Some(err) = error.filter(|text| !text.is_empty()) {
        return err.to_owned();
    }
    if models.is_empty() {
        i18n::fl("settings-list-models-empty")
    } else {
        String::new()
    }
}

#[must_use]
pub fn filtered_provider_models<'a>(models: &'a [String], query: &str) -> Vec<&'a str> {
    let q = query.trim();
    if q.is_empty() {
        return models.iter().map(String::as_str).collect();
    }
    let needle = q.to_ascii_lowercase();
    models
        .iter()
        .filter(|model| model.to_ascii_lowercase().contains(&needle))
        .map(String::as_str)
        .collect()
}

pub fn sync_search_tab(state: &mut DetailUiState) {
    if state.search.is_empty() {
        return;
    }
    if let Some(best) = best_search_tab(&state.search) {
        state.tab = best;
    }
}

#[must_use]
fn search_has_match(query: &str) -> bool {
    query.is_empty() || DetailTab::ALL.iter().any(|tab| tab.matches_search(query))
}

fn provider_plugin_ids(state: &DetailUiState) -> Vec<String> {
    let mut ids: Vec<String> = state
        .plugins
        .iter()
        .map(|plugin| plugin.plugin.clone())
        .filter(|id| is_provider_plugin_id(id))
        .collect();
    if is_provider_plugin_id(&state.chat_plugin) && !ids.iter().any(|id| id == &state.chat_plugin) {
        ids.push(state.chat_plugin.clone());
    }
    ids.sort();
    ids.dedup();
    ids
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ui.horizontal(|ui| {
        ui.label(i18n::fl("detail-search"));
        ui.text_edit_singleline(&mut state.search);
    });
    sync_search_tab(state);
    ui.horizontal_wrapped(|ui| {
        for tab in DetailTab::ALL {
            let label = tab.label();
            if !tab.matches_search(&state.search) {
                continue;
            }
            if ui.selectable_label(state.tab == tab, label).clicked() {
                state.select_tab(tab);
            }
        }
    });
    ui.separator();
    if !search_has_match(&state.search) {
        ui.label(i18n::fl("detail-search-empty"));
        return;
    }
    if state.tab != DetailTab::Home && !state.core_status.is_empty() {
        ui.label(&state.core_status);
    }
    match state.tab {
        DetailTab::Home => show_home(ui, state, client, rt, async_results),
        DetailTab::Companion => show_companion(ui, state, soul_id, client, rt, async_results),
        DetailTab::Conversation => show_conversation(ui, state, client, rt, async_results),
        DetailTab::Voice => show_voice(ui, state, local_settings, client, rt, async_results),
        DetailTab::Memory => show_memory(ui, state, soul_id, client, rt, async_results),
        DetailTab::Work => show_work(ui, state, soul_id, client, rt, async_results),
        DetailTab::Connections => show_connections(ui, state, client, rt, async_results),
        DetailTab::System => show_system(ui, state, local_settings, client, rt, async_results),
        DetailTab::Log => show_log(ui, state),
    }
}

fn show_home(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    if !state.loaded.health {
        state.loaded.health = true;
        let health_client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::Health(health_client.health().await.map_err(|e| e.to_string()))
        });
        let plugins_client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListPlugins(
                plugins_client
                    .list_plugins()
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            )
        });
    }
    ui.heading(i18n::fl("detail-tab-home"));
    ui.label(format!("{}: {}", i18n::fl("home-health"), state.health));
    ui.label(format!(
        "{}: {}",
        i18n::fl("home-fibers"),
        state.plugins.len()
    ));
    ui.label(i18n::fl("home-fibers-hint"));
    let required = blocking_unconfigured(&state.unconfigured);
    if required.contains(&"chat") {
        ui.colored_label(egui::Color32::YELLOW, home_chat_next_step(state));
        if ui.button(i18n::fl("detail-tab-conversation")).clicked() {
            state.select_tab(DetailTab::Conversation);
        }
    } else {
        ui.label(i18n::fl("home-configured"));
    }
    let optional = optional_unconfigured(&state.unconfigured);
    if !optional.is_empty() {
        ui.label(format!(
            "{}: {}",
            i18n::fl("home-optional-tasks"),
            optional.join(", ")
        ));
        if optional
            .iter()
            .any(|task| *task == "classifier" || *task == "embedding" || *task == "proactive")
        {
            ui.label(i18n::fl("home-optional-internal"));
        }
        if optional.iter().any(|task| *task == "stt" || *task == "tts") {
            ui.label(i18n::fl("home-optional-voice"));
        }
    }
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("detail-tab-companion")).clicked() {
            state.select_tab(DetailTab::Companion);
        }
        if ui.button(i18n::fl("detail-tab-conversation")).clicked() {
            state.select_tab(DetailTab::Conversation);
        }
        if ui.button(i18n::fl("detail-tab-voice")).clicked() {
            state.select_tab(DetailTab::Voice);
        }
    });
}

#[expect(
    clippy::too_many_lines,
    reason = "companion tab owns import/export/activate"
)]
fn show_companion(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if !state.loaded.character {
        state.loaded.character = true;
        let soul_id = soul_id.to_owned();
        let client_soul = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadSoul(
                client_soul
                    .get_soul(&soul_id)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
        let client_list = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListCharacters(
                client_list
                    .list_characters()
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            )
        });
        let client_stage = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListOccupants(
                client_stage
                    .stage()
                    .await
                    .map(|s| s.occupants)
                    .map_err(|e| e.to_string()),
            )
        });
    }
    ui.heading(i18n::fl("detail-tab-companion"));
    if let Some(soul) = &state.soul {
        let name = if soul.display_name.is_empty() {
            soul.id.as_str()
        } else {
            soul.display_name.as_str()
        };
        ui.heading(name);
        if soul.id == soul_id {
            ui.label(i18n::fl("character-active"));
        }
    } else {
        ui.label(soul_id);
    }
    if ui.button(i18n::fl("character-import")).clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("enechar", &["enechar", "zip", "png", "charx"])
            .pick_file()
    {
        let path = path.display().to_string();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ImportCharacter(
                client
                    .import_character(&path)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }
    if ui.button(i18n::fl("character-export")).clicked() {
        match character_export_package_id(state.soul.as_ref()) {
            None => state.core_status = i18n::fl("character-export-need-package"),
            Some(export_id) => {
                let file_name = character_export_filename(&export_id);
                if let Some(path) = export_save_dialog(&file_name, "enechar", &["enechar", "zip"]) {
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::ExportCharacter(
                            async {
                                let value = client
                                    .export_character(&export_id)
                                    .await
                                    .map_err(|e| e.to_string())?;
                                let b64 = value
                                    .get("archive_b64")
                                    .and_then(Value::as_str)
                                    .ok_or_else(|| "export missing archive_b64".to_owned())?;
                                let bytes = base64::engine::general_purpose::STANDARD
                                    .decode(b64)
                                    .map_err(|e| e.to_string())?;
                                std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
                                Ok(())
                            }
                            .await,
                        )
                    });
                }
            }
        }
    }
    ui.label(i18n::fl("character-export-hint"));
    ui.collapsing(i18n::fl("character-advanced"), |ui| {
        if let Some(soul) = &state.soul {
            ui.label(format!("{}: {}", i18n::fl("character-soul"), soul.id));
            ui.label(format!(
                "{}: {}",
                i18n::fl("character-package"),
                soul.package_id.as_deref().unwrap_or("—")
            ));
            ui.label(format!(
                "{}: {}",
                i18n::fl("character-avatar"),
                soul.avatar_path
                    .clone()
                    .unwrap_or_else(|| i18n::fl("character-text-only"))
            ));
            ui.label(format!(
                "{}: {}",
                i18n::fl("character-body-id"),
                soul.body_ref.as_deref().unwrap_or("none")
            ));
        }
        ui.label(i18n::fl("character-occupants"));
        for occupant in &state.occupants {
            ui.label(format!(
                "{}  body={}  avatar={}",
                occupant.soul_id,
                occupant.body_id.as_deref().unwrap_or("none"),
                occupant.avatar_path.as_deref().unwrap_or("—")
            ));
        }
        ui.horizontal(|ui| {
            ui.label(i18n::fl("character-body-id"));
            ui.text_edit_singleline(&mut state.body_ref_draft);
            if ui.button(i18n::fl("character-apply-body")).clicked() {
                let body_ref = state.body_ref_draft.clone();
                let soul_id = soul_id.to_owned();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::PatchBody(
                        client
                            .patch_soul_body(
                                &soul_id,
                                &ene_api::SoulPatch {
                                    body_ref: Some(body_ref),
                                },
                            )
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
        });
        ui.label(i18n::fl("character-body-uuid-hint"));
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            for character in &state.characters {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}@{} ({}) {}",
                        character.id, character.version, character.kind, character.path
                    ));
                    if ui.button(i18n::fl("character-activate")).clicked() {
                        let id = character.id.clone();
                        let client = Arc::clone(client);
                        spawn_async(rt, async_results, async move {
                            AsyncOutcome::ActivateCharacter(
                                client
                                    .activate_character(&id)
                                    .await
                                    .map_err(|e| e.to_string()),
                            )
                        });
                    }
                });
            }
        });
}

fn show_conversation(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    ui.heading(i18n::fl("detail-tab-conversation"));
    task_row(ui, i18n::fl("settings-chat-plugin"), &mut state.chat_plugin);
    task_row(ui, i18n::fl("settings-chat-model"), &mut state.chat_model);
    task_row(
        ui,
        i18n::fl("settings-chat-base-url"),
        &mut state.chat_base_url,
    );
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-chat-api-key"));
        ui.add(egui::TextEdit::singleline(&mut state.chat_api_key).password(true));
    });
    if state.ai_chat_key_set {
        ui.label(i18n::fl("settings-chat-key-set"));
    } else if chat_setup_gap(state) == Some(ChatSetupGap::ApiKey) {
        ui.colored_label(
            egui::Color32::YELLOW,
            i18n::fl("settings-chat-key-required"),
        );
    }
    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        apply_ai_patch(state, client, rt, async_results);
    }
    ui.separator();
    if ui.button(i18n::fl("settings-list-models")).clicked() {
        let plugin = state.chat_plugin.clone();
        if plugin.is_empty() {
            state.core_status = i18n::fl("settings-patch-hint");
        } else {
            let base_url = state.chat_base_url.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                let result = client
                    .list_provider_models(&ene_api::ListProviderModelsRequest {
                        plugin,
                        task: "chat".to_owned(),
                        base_url,
                        api_key: String::new(),
                    })
                    .await
                    .map(|r| (r.models, r.error))
                    .map_err(|e| e.to_string());
                AsyncOutcome::ListProviderModels(result)
            });
        }
    }
    if !state.provider_models.is_empty() {
        ui.horizontal(|ui| {
            ui.label(i18n::fl("settings-model-filter"));
            ui.text_edit_singleline(&mut state.provider_model_filter);
            ui.label(format!(
                "{}: {}",
                i18n::fl("settings-models"),
                state.provider_models.len()
            ));
        });
        let query = state.provider_model_filter.clone();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("provider-models")
            .max_height(180.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for model in filtered_provider_models(&state.provider_models, &query) {
                    if ui
                        .selectable_label(state.chat_model == model, model)
                        .clicked()
                    {
                        picked = Some(model.to_owned());
                    }
                }
            });
        if let Some(model) = picked {
            state.chat_model = model;
        }
    }
    task_row(
        ui,
        i18n::fl("settings-classifier-plugin"),
        &mut state.classifier_plugin,
    );
    task_row(
        ui,
        i18n::fl("settings-embedding-plugin"),
        &mut state.embedding_plugin,
    );
    task_row(
        ui,
        i18n::fl("settings-proactive-plugin"),
        &mut state.proactive_plugin,
    );
}

fn show_voice(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    ui.heading(i18n::fl("detail-tab-voice"));
    task_row(ui, i18n::fl("settings-tts-plugin"), &mut state.tts_plugin);
    task_row(ui, i18n::fl("settings-stt-plugin"), &mut state.stt_plugin);
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-mic-device"));
        ui.text_edit_singleline(&mut local_settings.mic_device);
    });
    ui.checkbox(
        &mut local_settings.caption_enabled,
        i18n::fl("settings-captions"),
    );
    ui.checkbox(
        &mut local_settings.spotlight_enabled,
        i18n::fl("settings-spotlight"),
    );
    if ui.button(i18n::fl("settings-open-spotlight")).clicked() {
        state.open_spotlight = true;
    }
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-caption-position"));
        let current = if crate::surface::caption::POSITIONS
            .contains(&local_settings.caption_position.as_str())
        {
            local_settings.caption_position.as_str()
        } else {
            "bottom"
        };
        egui::ComboBox::from_id_salt("caption-position")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for position in crate::surface::caption::POSITIONS {
                    ui.selectable_value(
                        &mut local_settings.caption_position,
                        position.to_owned(),
                        position,
                    );
                }
            });
    });
    ui.checkbox(
        &mut local_settings.caption_pinned,
        i18n::fl("settings-caption-pin"),
    );
    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        apply_ai_patch(state, client, rt, async_results);
    }
    if ui.button(i18n::fl("settings-save-local")).clicked() {
        state.save_local_pending = true;
    }
}

fn show_memory(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if !state.loaded.memory {
        state.loaded.memory = true;
        let soul_id_mem = soul_id.to_owned();
        let client_m = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListMemories(
                client_m
                    .list_memories(&soul_id_mem, None)
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            )
        });
        let soul_id_pending = soul_id.to_owned();
        let client_p = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListPendingMemories(
                client_p
                    .list_pending_memories(&soul_id_pending)
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            )
        });
    }
    if ui.button(i18n::fl("memory-refresh")).clicked() {
        state.loaded.memory = false;
    }
    ui.heading(i18n::fl("memory-candidates"));
    if state.pending_memories.is_empty() {
        ui.label(i18n::fl("memory-pending-empty"));
    }
    for memory in &state.pending_memories {
        ui.group(|ui| {
            ui.label(format!(
                "{} [{}] ({})",
                memory.title, memory.kind, memory.scope
            ));
            ui.label(&memory.content);
            ui.horizontal(|ui| {
                if ui.button(i18n::fl("memory-accept")).clicked() {
                    resolve_memory(&memory.id, true, client, rt, async_results);
                }
                if ui.button(i18n::fl("memory-reject")).clicked() {
                    resolve_memory(&memory.id, false, client, rt, async_results);
                }
            });
        });
    }
    ui.heading(i18n::fl("detail-tab-memory"));
    if state.memories.is_empty() {
        ui.label(i18n::fl("memory-empty"));
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for memory in &state.memories {
            ui.group(|ui| {
                ui.label(format!(
                    "{} [{}] ({})",
                    memory.title, memory.kind, memory.scope
                ));
                ui.label(&memory.content);
                if ui.button(i18n::fl("memory-delete")).clicked() {
                    let id = memory.id.clone();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::DeleteMemory {
                            id: id.clone(),
                            result: client.delete_memory(&id).await.map_err(|e| e.to_string()),
                        }
                    });
                }
            });
        }
    });
}

fn resolve_memory(
    id: &str,
    accept: bool,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let id = id.to_owned();
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        AsyncOutcome::ResolveMemory {
            id: id.clone(),
            result: client
                .resolve_memory_candidate(&id, accept)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    });
}

#[expect(
    clippy::too_many_lines,
    reason = "work tab owns jobs, schedules, and session actions"
)]
fn show_work(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if !state.loaded.jobs {
        state.loaded.jobs = true;
        let soul_id = soul_id.to_owned();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = async {
                let jobs = client.list_jobs(Some(&soul_id)).await?.items;
                let schedules = client.list_schedules().await?.items;
                Ok((jobs, schedules))
            }
            .await
            .map_err(|e: ene_api::ApiError| e.to_string());
            AsyncOutcome::ListJobs(result)
        });
    }
    if ui.button(i18n::fl("jobs-refresh")).clicked() {
        state.loaded.jobs = false;
    }
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("jobs-fork")).clicked() {
            let session_id = state.session_id.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::ForkSession(
                    client
                        .fork_session(&session_id)
                        .await
                        .map(|s| s.id)
                        .map_err(|e| e.to_string()),
                )
            });
        }
        if ui.button(i18n::fl("jobs-compact")).clicked() {
            let session_id = state.session_id.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::CompactSession(
                    client
                        .compact(&session_id)
                        .await
                        .map(|r| r.entry_id.to_string())
                        .map_err(|e| e.to_string()),
                )
            });
        }
        if ui.button(i18n::fl("jobs-export")).clicked()
            && let Some(path) = export_save_dialog(
                &session_export_filename(&state.session_id),
                "json",
                &["json"],
            )
        {
            let session_id = state.session_id.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::ExportSession(
                    async {
                        let value = client
                            .export_session(&session_id)
                            .await
                            .map_err(|e| e.to_string())?;
                        let pretty =
                            serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
                        std::fs::write(&path, pretty).map_err(|e| e.to_string())?;
                        Ok(())
                    }
                    .await,
                )
            });
        }
    });
    ui.label(i18n::fl("jobs-export-hint"));
    ui.heading(i18n::fl("jobs-active"));
    if state.jobs.is_empty() {
        ui.label(i18n::fl("jobs-empty"));
    }
    for job in &state.jobs {
        ui.horizontal(|ui| {
            ui.label(format!("{} [{}] {}", job.title, job.status, job.id));
            if ui.button(i18n::fl("jobs-cancel")).clicked() {
                let id = job.id.clone();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::CancelJob {
                        id: id.clone(),
                        result: client
                            .cancel_job(&id)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    }
                });
            }
        });
    }
    ui.heading(i18n::fl("jobs-schedules"));
    for schedule in &state.schedules {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} ({}) enabled={}",
                schedule.name, schedule.spec, schedule.enabled
            ));
            let label = if schedule.enabled {
                i18n::fl("schedule-disable")
            } else {
                i18n::fl("schedule-enable")
            };
            if ui.button(label).clicked() {
                let id = schedule.id.clone();
                let enabled = !schedule.enabled;
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::ToggleSchedule {
                        id: id.clone(),
                        enabled,
                        result: client
                            .patch_schedule(&id, enabled)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    }
                });
            }
        });
    }
}

fn show_connections(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    if !state.loaded.plugins {
        state.loaded.plugins = true;
        let client_list = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListPlugins(
                client_list
                    .list_plugins()
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            )
        });
        let client_mcp = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadMcp(
                async {
                    let doc = client_mcp.mcp().await.map_err(|e| e.to_string())?;
                    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
                }
                .await,
            )
        });
    }
    ui.heading(i18n::fl("detail-tab-connections"));
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-plugins-profile"));
        for profile in ["desktop", "minimal", "headless"] {
            if ui
                .selectable_label(state.plugins_profile == profile, profile)
                .clicked()
            {
                profile.clone_into(&mut state.plugins_profile);
            }
        }
        if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
            let profile = state.plugins_profile.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::ApplyCoreSettings(
                    client
                        .patch_settings(&serde_json::json!({ "plugins": { "profile": profile } }))
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                )
            });
        }
    });
    ui.label(i18n::fl("plugins-no-enable-map"));
    for plugin in &state.plugins {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} ({}) — {}",
                plugin.plugin, plugin.state, plugin.row_id
            ));
            if ui.button(i18n::fl("plugins-restart")).clicked() {
                let id = plugin.row_id.clone();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::RestartPlugin {
                        id: id.clone(),
                        result: client
                            .restart_plugin(&id)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    }
                });
            }
        });
    }
    show_provider_assets(ui, state, client, rt, async_results);
    ui.separator();
    ui.heading(i18n::fl("plugins-mcp"));
    ui.add(
        egui::TextEdit::multiline(&mut state.mcp_json)
            .desired_width(f32::INFINITY)
            .desired_rows(8),
    );
    if ui.button(i18n::fl("plugins-mcp-save")).clicked() {
        let text = state.mcp_json.clone();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::SaveMcp(
                async {
                    let doc: ene_api::McpDocument = serde_json::from_str(&text)
                        .map_err(|e| format!("invalid MCP JSON: {e}"))?;
                    client.put_mcp(&doc).await.map_err(|e| e.to_string())?;
                    Ok(())
                }
                .await,
            )
        });
    }
}

fn show_provider_assets(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if !is_provider_plugin_id(&state.provider_assets_plugin) {
        let next = default_provider_assets_plugin(&state.chat_plugin, &state.plugins);
        if state.provider_assets_plugin != next {
            state.provider_assets_plugin = next;
            state.loaded.provider_assets = false;
            state.provider_assets.clear();
        }
    }
    if is_provider_plugin_id(&state.provider_assets_plugin) && !state.loaded.provider_assets {
        state.loaded.provider_assets = true;
        let plugin = state.provider_assets_plugin.clone();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListProviderAssets(
                client
                    .list_provider_assets(&ene_api::ListProviderAssetsRequest { plugin })
                    .await
                    .map(|r| r.assets)
                    .map_err(|e| e.to_string()),
            )
        });
    }
    ui.heading(i18n::fl("plugins-assets"));
    if !state.connections_status.is_empty() {
        ui.colored_label(egui::Color32::YELLOW, &state.connections_status);
    }
    let ids = provider_plugin_ids(state);
    if ids.is_empty() {
        ui.label(i18n::fl("plugins-assets-need-provider"));
        return;
    }
    ui.horizontal(|ui| {
        ui.label(i18n::fl("plugins-assets-plugin"));
        let previous = state.provider_assets_plugin.clone();
        egui::ComboBox::from_id_salt("provider-assets-plugin")
            .selected_text(&previous)
            .show_ui(ui, |ui| {
                for id in &ids {
                    ui.selectable_value(&mut state.provider_assets_plugin, id.clone(), id);
                }
            });
        if state.provider_assets_plugin != previous {
            state.loaded.provider_assets = false;
            state.provider_assets.clear();
        }
        if ui.button(i18n::fl("plugins-assets-load")).clicked()
            && is_provider_plugin_id(&state.provider_assets_plugin)
        {
            state.loaded.provider_assets = false;
        }
    });
    for asset in &state.provider_assets {
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} ({}) — {}",
                asset.label,
                asset.kind,
                if asset.active {
                    i18n::fl("plugins-assets-active")
                } else {
                    i18n::fl("plugins-assets-inactive")
                }
            ));
            if !asset.active && ui.button(i18n::fl("plugins-assets-activate")).clicked() {
                let plugin = state.provider_assets_plugin.clone();
                let asset_id = asset.id.clone();
                let version = asset.active_version.clone();
                let outcome_id = asset_id.clone();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::SetActiveProviderAsset {
                        asset_id: outcome_id,
                        result: client
                            .set_active_provider_asset(&ene_api::SetActiveProviderAssetRequest {
                                plugin,
                                asset_id,
                                version,
                            })
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string()),
                    }
                });
            }
            if ui.button(i18n::fl("plugins-assets-install")).clicked() {
                let plugin = state.provider_assets_plugin.clone();
                let asset_id = asset.id.clone();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::InstallProviderAsset {
                        asset_id: asset_id.clone(),
                        result: client
                            .install_provider_asset(&ene_api::InstallProviderAssetRequest {
                                plugin,
                                asset_id,
                                version: None,
                                variant: None,
                            })
                            .await
                            .map(|r| r.job_id)
                            .map_err(|e| e.to_string()),
                    }
                });
            }
        });
    }
}

fn show_system(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    egui::ScrollArea::vertical().show(ui, |ui| {
        show_system_inner(ui, state, local_settings, client, rt, async_results);
    });
}

#[expect(
    clippy::too_many_lines,
    reason = "system tab stacks local settings, backup, and JSON"
)]
fn show_system_inner(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ui.heading(i18n::fl("settings-local"));
    egui::Grid::new("stage-local-settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(i18n::fl("settings-theme"));
            egui::ComboBox::from_id_salt("theme")
                .selected_text(&local_settings.theme)
                .show_ui(ui, |ui| {
                    for theme in ["system", "dark", "light"] {
                        ui.selectable_value(&mut local_settings.theme, theme.to_owned(), theme);
                    }
                });
            ui.end_row();
            ui.label(i18n::fl("settings-language"));
            let language_label = if local_settings.language.is_empty() {
                i18n::fl("settings-language-system")
            } else {
                local_settings.language.clone()
            };
            egui::ComboBox::from_id_salt("language")
                .selected_text(language_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut local_settings.language,
                        String::new(),
                        i18n::fl("settings-language-system"),
                    );
                    ui.selectable_value(&mut local_settings.language, "ja".to_owned(), "ja");
                    ui.selectable_value(&mut local_settings.language, "en-US".to_owned(), "en-US");
                });
            ui.end_row();
            ui.label(i18n::fl("settings-core-lifetime"));
            egui::ComboBox::from_id_salt("core-lifetime")
                .selected_text(&local_settings.core_lifetime)
                .show_ui(ui, |ui| {
                    for value in ["app", "detached"] {
                        ui.selectable_value(
                            &mut local_settings.core_lifetime,
                            value.to_owned(),
                            value,
                        );
                    }
                });
            ui.end_row();
            ui.label(i18n::fl("settings-graphics-quality"));
            ui.text_edit_singleline(&mut local_settings.graphics_quality);
            ui.end_row();
            ui.label(i18n::fl("settings-always-on-top"));
            ui.checkbox(&mut local_settings.always_on_top, "");
            ui.end_row();
            ui.label(i18n::fl("settings-click-through"));
            ui.checkbox(&mut local_settings.overlay_click_through, "")
                .on_hover_text(i18n::fl("settings-click-through-hint"));
            ui.end_row();
            ui.label(i18n::fl("settings-model-scale"));
            ui.add(egui::Slider::new(
                &mut local_settings.model_scale,
                0.3..=2.0,
            ));
            ui.end_row();
            ui.label(i18n::fl("settings-look-at"));
            ui.add(egui::Slider::new(
                &mut local_settings.look_at_strength,
                0.0..=1.0,
            ));
            ui.end_row();
            ui.label(i18n::fl("settings-beat-sync"));
            ui.checkbox(&mut local_settings.beat_sync, "");
            ui.end_row();
            ui.label(i18n::fl("settings-beat-sync-device"));
            ui.text_edit_singleline(&mut local_settings.beat_sync_device);
            ui.end_row();
        });
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("settings-save-local")).clicked() {
            state.save_local_pending = true;
        }
        if ui.button(i18n::fl("settings-discard-local")).clicked() {
            *local_settings = crate::settings::load_desktop_settings();
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("system-backup")).clicked() {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::Backup(
                    client
                        .backup()
                        .await
                        .map(|b| (b.id, b.path))
                        .map_err(|e| e.to_string()),
                )
            });
        }
        if ui.button(i18n::fl("system-usage")).clicked() {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::Usage(client.usage(None).await.map_err(|e| e.to_string()))
            });
        }
        if ui.button(i18n::fl("system-spans")).clicked() {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::DiagSpans(
                    client
                        .diag_spans()
                        .await
                        .map(|p| p.items)
                        .map_err(|e| e.to_string()),
                )
            });
        }
        if ui.button(i18n::fl("system-schema")).clicked() {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::LoadSchema(
                    client
                        .settings_schema()
                        .await
                        .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()))
                        .map_err(|e| e.to_string()),
                )
            });
        }
    });
    ui.label(i18n::fl("system-reload-hint"));
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("settings-reload-core")).clicked() {
            state.loaded.settings = false;
            ensure_settings(state, client, rt, async_results);
        }
        ui.label(i18n::fl("system-restore-id"));
        ui.text_edit_singleline(&mut state.restore_id);
        ui.checkbox(
            &mut state.restore_confirm,
            i18n::fl("system-restore-confirm"),
        );
        if ui
            .add_enabled(
                state.restore_confirm && !state.restore_id.is_empty(),
                egui::Button::new(i18n::fl("system-restore")),
            )
            .clicked()
        {
            let id = state.restore_id.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::Restore(
                    client
                        .restore(&ene_api::RestoreRequest { id })
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                )
            });
        }
    });
    if !state.usage_text.is_empty() {
        ui.label(&state.usage_text);
    }
    if !state.spans_text.is_empty() {
        ui.label(&state.spans_text);
    }
    ui.heading(i18n::fl("settings-core-json"));
    ui.add(
        egui::TextEdit::multiline(&mut state.core_settings_text)
            .desired_width(f32::INFINITY)
            .desired_rows(8),
    );
    ui.label(i18n::fl("settings-core-patch"));
    ui.add(
        egui::TextEdit::multiline(&mut state.core_patch_text)
            .desired_width(f32::INFINITY)
            .desired_rows(4)
            .hint_text(i18n::fl("settings-patch-hint")),
    );
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("settings-apply-patch")).clicked() {
            let text = state.core_patch_text.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::ApplyCoreSettings(
                    async {
                        let patch: Value = serde_json::from_str(&text)
                            .map_err(|e| format!("invalid JSON: {e}"))?;
                        client
                            .patch_settings(&patch)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(())
                    }
                    .await,
                )
            });
        }
    });
    if !state.schema_json.is_empty() {
        ui.heading(i18n::fl("system-advanced"));
        ui.add(
            egui::TextEdit::multiline(&mut state.schema_json)
                .desired_width(f32::INFINITY)
                .desired_rows(8),
        );
    }
}

#[must_use]
fn log_empty_copy(entry_count: usize) -> Option<String> {
    (entry_count == 0).then(|| i18n::fl("log-empty"))
}

fn show_log(ui: &mut egui::Ui, state: &DetailUiState) {
    ui.heading(i18n::fl("detail-tab-log"));
    if let Some(empty) = log_empty_copy(state.log.len()) {
        ui.label(empty);
        ui.label(i18n::fl("log-empty-hint"));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for entry in &state.log {
            let prefix = match entry.kind {
                LogKind::Thinking => "thinking",
                LogKind::Inner => "inner",
                LogKind::Tool => "tool",
                LogKind::Session => "session",
                LogKind::Job => "job",
                LogKind::Affect => "affect",
            };
            ui.add(egui::Label::new(format!("[{prefix}] {}", entry.text)).wrap());
        }
    });
}

fn character_export_package_id(soul: Option<&SoulView>) -> Option<String> {
    let package = soul.and_then(|soul| soul.package_id.clone())?;
    let id = package
        .split_once('@')
        .map(|(pkg, _)| pkg.to_owned())
        .unwrap_or(package);
    if id.is_empty() { None } else { Some(id) }
}

#[must_use]
fn safe_export_stem(name: &str) -> String {
    let mut stem = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            stem.push(ch);
        } else if !stem.is_empty() && !stem.ends_with('_') {
            stem.push('_');
        }
    }
    let stem = stem.trim_matches('_');
    if stem.is_empty() {
        "export".to_owned()
    } else {
        stem.chars().take(64).collect()
    }
}

#[must_use]
fn character_export_filename(package_or_name: &str) -> String {
    format!("{}.enechar", safe_export_stem(package_or_name))
}

#[must_use]
fn session_export_filename(session_id: &str) -> String {
    format!("{}.json", safe_export_stem(session_id))
}

#[must_use]
fn default_export_dir() -> PathBuf {
    default_export_dir_from(
        std::env::var_os("XDG_DOCUMENTS_DIR"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
        directories::UserDirs::new().and_then(|dirs| dirs.document_dir().map(PathBuf::from)),
    )
}

#[must_use]
fn default_export_dir_from(
    xdg_documents: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    platform_documents: Option<PathBuf>,
) -> PathBuf {
    if let Some(docs) = xdg_documents {
        let path = PathBuf::from(docs);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    if let Some(docs) = platform_documents.filter(|path| !path.as_os_str().is_empty()) {
        return docs;
    }
    if let Some(home) = home {
        let home = PathBuf::from(home);
        for name in ["Documents", "Downloads"] {
            let candidate = home.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
        if !home.as_os_str().is_empty() {
            return home;
        }
    }
    std::env::temp_dir()
}

fn export_save_dialog(file_name: &str, filter_name: &str, extensions: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(default_export_dir())
        .set_file_name(file_name)
        .add_filter(filter_name, extensions)
        .save_file()
}

fn task_row(ui: &mut egui::Ui, label: String, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.text_edit_singleline(value);
    });
}

fn ensure_settings(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if state.loaded.settings {
        return;
    }
    state.loaded.settings = true;
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        AsyncOutcome::LoadCoreSettings(
            client
                .settings()
                .await
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()))
                .map_err(|e| e.to_string()),
        )
    });
}

fn apply_ai_patch(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let mut chat = serde_json::json!({
        "plugin": state.chat_plugin,
        "model": state.chat_model,
        "base_url": state.chat_base_url,
    });
    if !state.chat_api_key.is_empty()
        && let Some(obj) = chat.as_object_mut()
    {
        obj.insert(
            "api_key".to_owned(),
            Value::String(state.chat_api_key.clone()),
        );
        state.chat_api_key.clear();
    }
    let patch = serde_json::json!({
        "ai": {
            "tasks": {
                "chat": chat,
                "classifier": { "plugin": state.classifier_plugin },
                "embedding": { "plugin": state.embedding_plugin },
                "proactive": { "plugin": state.proactive_plugin },
                "tts": { "plugin": state.tts_plugin },
                "stt": { "plugin": state.stt_plugin },
            }
        }
    });
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        AsyncOutcome::ApplyCoreSettings(
            client
                .patch_settings(&patch)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        )
    });
}

fn spawn_async(
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
    task: impl std::future::Future<Output = AsyncOutcome> + Send + 'static,
) {
    let results = Arc::clone(async_results);
    rt.spawn(async move {
        let outcome = task.await;
        results.lock().push(outcome);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_core_fields_reads_effective_tasks_and_profile() {
        let json = r#"{
            "overlay": {},
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": { "plugin": "openai", "model": "gpt", "base_url": "https://example.invalid/v1" },
                        "classifier": { "plugin": "echo" }
                    }
                },
                "plugins": { "profile": "desktop" },
                "ai_chat_key_set": true
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(state.chat_plugin, "openai");
        assert_eq!(state.chat_model, "gpt");
        assert_eq!(state.chat_base_url, "https://example.invalid/v1");
        assert!(state.ai_chat_key_set);
        assert_eq!(state.plugins_profile, "desktop");
        assert!(!state.unconfigured.iter().any(|task| task == "chat"));
        assert!(state.unconfigured.iter().any(|task| task == "classifier"));
        assert!(state.unconfigured.iter().any(|task| task == "stt"));
        assert!(blocking_unconfigured(&state.unconfigured).is_empty());
        assert!(optional_unconfigured(&state.unconfigured).contains(&"classifier"));
    }

    #[test]
    fn openai_compat_without_key_is_not_chat_ready() {
        let json = r#"{
            "overlay": {},
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": {
                            "plugin": "provider.openai_compat",
                            "model": "openai/gpt-4o-mini",
                            "base_url": "https://example.invalid/v1"
                        }
                    }
                },
                "ai_chat_key_set": false,
                "providers": [
                    { "id": "provider.openai_compat", "needs_key": true },
                    { "id": "provider.gguf", "needs_key": false }
                ]
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(chat_setup_gap(&state), Some(ChatSetupGap::ApiKey));
        assert!(blocking_unconfigured(&state.unconfigured).contains(&"chat"));
        assert_eq!(home_chat_next_step(&state), i18n::fl("home-next-chat-key"));
    }

    #[test]
    fn gguf_without_key_is_chat_ready() {
        let json = r#"{
            "overlay": {},
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": { "plugin": "provider.gguf", "model": "local.gguf" }
                    }
                },
                "ai_chat_key_set": false,
                "providers": [
                    { "id": "provider.gguf", "needs_key": false }
                ]
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(chat_setup_gap(&state), None);
        assert!(blocking_unconfigured(&state.unconfigured).is_empty());
    }

    #[test]
    fn backup_keyword_keeps_system_tab_visible() {
        assert!(DetailTab::System.matches_search("backup"));
        assert!(DetailTab::System.matches_search("restore"));
        assert!(!DetailTab::Home.matches_search("backup"));
    }

    #[test]
    fn search_vo_switches_from_home_to_voice() {
        let mut state = DetailUiState {
            tab: DetailTab::Home,
            search: "vo".to_owned(),
            ..DetailUiState::default()
        };
        sync_search_tab(&mut state);
        assert_eq!(state.tab, DetailTab::Voice);
        assert!(!DetailTab::Home.matches_search("vo"));
        assert!(
            DetailTab::Voice
                .search_rank("vo")
                .is_some_and(|rank| rank < 4)
        );
        let mut from_conversation = DetailUiState {
            tab: DetailTab::Conversation,
            search: "vo".to_owned(),
            ..DetailUiState::default()
        };
        sync_search_tab(&mut from_conversation);
        assert_eq!(from_conversation.tab, DetailTab::Voice);
    }

    #[test]
    fn explicit_tab_click_clears_search_so_home_cannot_trap_navigation() {
        let mut state = DetailUiState {
            tab: DetailTab::Home,
            search: "home".to_owned(),
            ..DetailUiState::default()
        };
        sync_search_tab(&mut state);
        assert_eq!(state.tab, DetailTab::Home);
        state.select_tab(DetailTab::Conversation);
        sync_search_tab(&mut state);
        assert_eq!(state.tab, DetailTab::Conversation);
        assert!(state.search.is_empty());
    }

    #[test]
    fn default_provider_assets_skips_tool_plugins() {
        let plugins = vec![
            PluginView {
                row_id: "1".into(),
                plugin: "tool.utility".into(),
                state: "ready".into(),
                wait_reason: None,
            },
            PluginView {
                row_id: "2".into(),
                plugin: "provider.openai_compat".into(),
                state: "ready".into(),
                wait_reason: None,
            },
        ];
        assert_eq!(
            default_provider_assets_plugin("echo", &plugins),
            "provider.openai_compat"
        );
        assert_eq!(
            default_provider_assets_plugin("provider.gguf", &plugins),
            "provider.gguf"
        );
        assert!(!is_provider_plugin_id("tool.utility"));
        assert!(!is_provider_plugin_id("provider."));
        assert!(is_provider_plugin_id("provider.openai_compat"));
    }

    #[test]
    fn list_models_status_prefers_error_then_empty_hint() {
        assert_eq!(
            list_models_status(&[], Some("unauthorized")),
            "unauthorized"
        );
        assert_eq!(
            list_models_status(&[], None),
            i18n::fl("settings-list-models-empty")
        );
        assert!(list_models_status(&["gpt".into()], None).is_empty());
    }

    #[test]
    fn filtered_provider_models_keeps_apply_reachable_by_narrowing() {
        let models = vec![
            "openai/gpt-4o".into(),
            "openai/gpt-4o-mini".into(),
            "anthropic/claude".into(),
        ];
        assert_eq!(filtered_provider_models(&models, "").len(), 3);
        assert_eq!(
            filtered_provider_models(&models, "mini"),
            vec!["openai/gpt-4o-mini"]
        );
        assert!(filtered_provider_models(&models, "nope").is_empty());
    }

    #[test]
    fn log_empty_copy_is_resolved() {
        assert_eq!(log_empty_copy(1), None);
        let empty = log_empty_copy(0).expect("placeholder");
        assert_ne!(empty, "log-empty");
        assert_ne!(i18n::fl("log-empty-hint"), "log-empty-hint");
    }

    #[test]
    fn export_names_are_safe_and_typed() {
        assert_eq!(
            character_export_filename("char.alicia@1.0.0"),
            "char_alicia_1_0_0.enechar"
        );
        assert_eq!(session_export_filename(""), "export.json");
        assert_eq!(safe_export_stem("???"), "export");
        let dir = default_export_dir_from(Some("/tmp/ene-docs".into()), None, None);
        assert_eq!(dir, PathBuf::from("/tmp/ene-docs"));
        let platform = default_export_dir_from(
            None,
            Some("/tmp/ene-home".into()),
            Some(PathBuf::from("/tmp/ene-xdg-docs")),
        );
        assert_eq!(platform, PathBuf::from("/tmp/ene-xdg-docs"));
        assert!(character_export_package_id(None).is_none());
        let unbound = SoulView {
            id: "soul".into(),
            character_ref: String::new(),
            display_name: "Alicia".into(),
            body_ref: None,
            voice_ref: None,
            mood_label: String::new(),
            package_id: None,
            avatar_path: None,
        };
        assert!(character_export_package_id(Some(&unbound)).is_none());
        let mut packaged = unbound.clone();
        packaged.package_id = Some("char.alicia@1.0.0".into());
        assert_eq!(
            character_export_package_id(Some(&packaged)).as_deref(),
            Some("char.alicia")
        );
    }
}
