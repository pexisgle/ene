//! Detail window: eight new-core IA sections plus a session log.

use std::collections::HashMap;
use std::sync::Arc;

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
    pub classifier_plugin: String,
    pub embedding_plugin: String,
    pub proactive_plugin: String,
    pub tts_plugin: String,
    pub stt_plugin: String,
    pub plugins_profile: String,
    pub core_status: String,
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
    pub mcp_json: String,
    pub schema_json: String,
    pub usage_text: String,
    pub spans_text: String,
    pub save_local_pending: bool,
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
}

pub fn parse_core_fields(json: &str, state: &mut DetailUiState) {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return;
    };
    let effective = value.get("effective").unwrap_or(&value);
    state.chat_plugin = nested_string(effective, &["ai", "tasks", "chat", "plugin"]);
    state.chat_model = nested_string(effective, &["ai", "tasks", "chat", "model"]);
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
        if plugin.is_empty() || plugin == "echo" {
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
    ui.horizontal_wrapped(|ui| {
        for tab in DetailTab::ALL {
            let label = tab.label();
            if !state.search.is_empty()
                && !label
                    .to_ascii_lowercase()
                    .contains(&state.search.to_ascii_lowercase())
            {
                continue;
            }
            if ui.selectable_label(state.tab == tab, label).clicked() {
                state.tab = tab;
            }
        }
    });
    ui.separator();
    if !state.core_status.is_empty() {
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
    if state.unconfigured.is_empty() {
        ui.label(i18n::fl("home-configured"));
    } else {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!(
                "{}: {}",
                i18n::fl("home-unconfigured"),
                state.unconfigured.join(", ")
            ),
        );
    }
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("detail-tab-companion")).clicked() {
            state.tab = DetailTab::Companion;
        }
        if ui.button(i18n::fl("detail-tab-conversation")).clicked() {
            state.tab = DetailTab::Conversation;
        }
        if ui.button(i18n::fl("detail-tab-voice")).clicked() {
            state.tab = DetailTab::Voice;
        }
    });
}

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
        ui.label(format!("{}: {}", i18n::fl("character-soul"), soul.id));
        ui.label(format!(
            "{}: {}",
            i18n::fl("character-display"),
            soul.display_name
        ));
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
    if ui.button(i18n::fl("settings-list-models")).clicked() {
        let plugin = state.chat_plugin.clone();
        if plugin.is_empty() {
            state.core_status = i18n::fl("settings-patch-hint");
        } else {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                let result = client
                    .list_provider_models(&ene_api::ListProviderModelsRequest {
                        plugin,
                        task: "chat".to_owned(),
                        ..ene_api::ListProviderModelsRequest::default()
                    })
                    .await
                    .map(|r| r.models)
                    .map_err(|e| e.to_string());
                AsyncOutcome::ListProviderModels(result)
            });
        }
    }
    for model in &state.provider_models {
        if ui
            .selectable_label(state.chat_model == *model, model)
            .clicked()
        {
            state.chat_model.clone_from(model);
        }
    }
    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        apply_ai_patch(state, client, rt, async_results);
    }
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
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-caption-position"));
        ui.text_edit_singleline(&mut local_settings.caption_position);
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
    ui.heading(i18n::fl("jobs-active"));
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
    if state.provider_assets_plugin.is_empty() {
        if let Some(plugin) = state.plugins.first() {
            state.provider_assets_plugin = plugin.plugin.clone();
        } else if !state.chat_plugin.is_empty() {
            state.provider_assets_plugin.clone_from(&state.chat_plugin);
        }
    }
    if !state.provider_assets_plugin.is_empty() && !state.loaded.provider_assets {
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
    ui.horizontal(|ui| {
        ui.label(i18n::fl("plugins-assets-plugin"));
        ui.text_edit_singleline(&mut state.provider_assets_plugin);
        if ui.button(i18n::fl("plugins-assets-load")).clicked() {
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
            ui.text_edit_singleline(&mut local_settings.language);
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
                        .map(|b| b.path)
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
        if ui.button(i18n::fl("settings-reload-core")).clicked() {
            state.loaded.settings = false;
            ensure_settings(state, client, rt, async_results);
        }
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

fn show_log(ui: &mut egui::Ui, state: &DetailUiState) {
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
            ui.label(format!("[{prefix}] {}", entry.text));
        }
    });
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
    state: &DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let patch = serde_json::json!({
        "ai": {
            "tasks": {
                "chat": { "plugin": state.chat_plugin, "model": state.chat_model },
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
                        "chat": { "plugin": "openai", "model": "gpt" },
                        "classifier": { "plugin": "echo" }
                    }
                },
                "plugins": { "profile": "desktop" }
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(state.chat_plugin, "openai");
        assert_eq!(state.chat_model, "gpt");
        assert_eq!(state.plugins_profile, "desktop");
        assert!(!state.unconfigured.iter().any(|task| task == "chat"));
        assert!(state.unconfigured.iter().any(|task| task == "classifier"));
        assert!(state.unconfigured.iter().any(|task| task == "stt"));
    }
}
