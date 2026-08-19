//! Detail viewport with log, settings, memory, character, jobs, and plugins tabs.

use std::sync::Arc;

use eframe::egui::{self, ScrollArea, TextEdit};
use ene_api::{
    ApiClient, CharacterView, JobView, MemoryView, PluginView, ScheduleView, SoulView,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::runtime::Handle;

use crate::i18n;
use crate::settings::{save_desktop_settings, DesktopSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailTab {
    #[default]
    Log,
    Settings,
    Memory,
    Character,
    Jobs,
    Plugins,
}

impl DetailTab {
    const ALL: [DetailTab; 6] = [
        DetailTab::Log,
        DetailTab::Settings,
        DetailTab::Memory,
        DetailTab::Character,
        DetailTab::Jobs,
        DetailTab::Plugins,
    ];

    fn label(self) -> String {
        match self {
            DetailTab::Log => i18n::fl("detail-tab-log"),
            DetailTab::Settings => i18n::fl("detail-tab-settings"),
            DetailTab::Memory => i18n::fl("detail-tab-memory"),
            DetailTab::Character => i18n::fl("detail-tab-character"),
            DetailTab::Jobs => i18n::fl("detail-tab-jobs"),
            DetailTab::Plugins => i18n::fl("detail-tab-plugins"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Thinking,
    Tool,
    Session,
    Job,
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
    pub log: Vec<LogEntry>,
    pub core_settings_text: String,
    pub core_patch_text: String,
    pub chat_plugin: String,
    pub chat_model: String,
    pub tts_plugin: String,
    pub stt_plugin: String,
    pub core_status: String,
    pub memories: Vec<MemoryView>,
    pub soul: Option<SoulView>,
    pub characters: Vec<CharacterView>,
    pub body_ref_draft: String,
    pub jobs: Vec<JobView>,
    pub schedules: Vec<ScheduleView>,
    pub plugins: Vec<PluginView>,
    pub mcp_json: String,
    loaded: DetailLoaded,
}

#[derive(Default)]
struct DetailLoaded {
    settings: bool,
    memory: bool,
    character: bool,
    jobs: bool,
    plugins: bool,
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
    state.chat_plugin = nested_string(&value, &["ai", "tasks", "chat", "plugin"]);
    state.chat_model = nested_string(&value, &["ai", "tasks", "chat", "model"]);
    state.tts_plugin = nested_string(&value, &["ai", "tasks", "tts", "plugin"]);
    state.stt_plugin = nested_string(&value, &["ai", "tasks", "stt", "plugin"]);
}

fn nested_string(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(key) else {
            return String::new();
        };
        current = next;
    }
    current.as_str().unwrap_or("").to_owned()
}

#[expect(clippy::too_many_arguments, reason = "detail UI coordinates async actions")]
pub fn show(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
) {
    ui.horizontal(|ui| {
        for tab in DetailTab::ALL {
            if ui.selectable_label(state.tab == tab, tab.label()).clicked() {
                state.tab = tab;
            }
        }
    });
    ui.separator();
    if !state.core_status.is_empty() {
        ui.label(&state.core_status);
    }

    match state.tab {
        DetailTab::Log => show_log(ui, state),
        DetailTab::Settings => show_settings(
            ui,
            state,
            local_settings,
            client,
            rt,
            async_results,
        ),
        DetailTab::Memory => show_memory(ui, state, soul_id, client, rt, async_results),
        DetailTab::Character => show_character(ui, state, soul_id, client, rt, async_results),
        DetailTab::Jobs => show_jobs(ui, state, soul_id, client, rt, async_results),
        DetailTab::Plugins => show_plugins(ui, state, client, rt, async_results),
    }
}

fn show_log(ui: &mut egui::Ui, state: &DetailUiState) {
    ScrollArea::vertical().show(ui, |ui| {
        for entry in &state.log {
            let prefix = match entry.kind {
                LogKind::Thinking => "thinking",
                LogKind::Tool => "tool",
                LogKind::Session => "session",
                LogKind::Job => "job",
            };
            ui.label(format!("[{prefix}] {}", entry.text));
        }
    });
}

fn show_settings(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
) {
    if !state.loaded.settings {
        state.loaded.settings = true;
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client
                .settings()
                .await
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()))
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::LoadCoreSettings(result)
        });
    }

    ui.heading(i18n::fl("settings-local"));
    egui::Grid::new("stage-local-settings").show(ui, |ui| {
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
                    ui.selectable_value(&mut local_settings.core_lifetime, value.to_owned(), value);
                }
            });
        ui.end_row();

        ui.label(i18n::fl("settings-captions"));
        ui.checkbox(&mut local_settings.caption_enabled, "");
        ui.end_row();

        ui.label(i18n::fl("settings-spotlight"));
        ui.checkbox(&mut local_settings.spotlight_enabled, "");
        ui.end_row();

        ui.label(i18n::fl("settings-always-on-top"));
        ui.checkbox(&mut local_settings.always_on_top, "");
        ui.end_row();

        ui.label(i18n::fl("settings-model-scale"));
        ui.add(egui::Slider::new(&mut local_settings.model_scale, 0.3..=2.0));
        ui.end_row();

        ui.label(i18n::fl("settings-look-at"));
        ui.add(egui::Slider::new(&mut local_settings.look_at_strength, 0.0..=1.0));
        ui.end_row();
    });

    if ui.button(i18n::fl("settings-save-local")).clicked() {
        let settings = local_settings.clone();
        spawn_async(rt, async_results, async move {
            let result = save_desktop_settings(&settings).map_err(|e| e.to_string());
            crate::app::AsyncOutcome::SaveLocalSettings(result)
        });
    }

    ui.separator();
    ui.heading(i18n::fl("settings-core"));
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-chat-plugin"));
        ui.text_edit_singleline(&mut state.chat_plugin);
    });
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-chat-model"));
        ui.text_edit_singleline(&mut state.chat_model);
    });
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-tts-plugin"));
        ui.text_edit_singleline(&mut state.tts_plugin);
    });
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-stt-plugin"));
        ui.text_edit_singleline(&mut state.stt_plugin);
    });

    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        let patch = build_core_patch(state);
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client
                .patch_settings(&patch)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::ApplyCoreSettings(result)
        });
    }

    ui.label(i18n::fl("settings-core-json"));
    ui.add(
        TextEdit::multiline(&mut state.core_settings_text)
            .desired_width(f32::INFINITY)
            .desired_rows(8),
    );
    ui.label(i18n::fl("settings-core-patch"));
    ui.add(
        TextEdit::multiline(&mut state.core_patch_text)
            .desired_width(f32::INFINITY)
            .desired_rows(4)
            .hint_text(i18n::fl("settings-patch-hint")),
    );
    ui.horizontal(|ui| {
        if ui.button(i18n::fl("settings-reload-core")).clicked() {
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                let result = client
                    .settings()
                    .await
                    .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()))
                    .map_err(|e| e.to_string());
                crate::app::AsyncOutcome::LoadCoreSettings(result)
            });
        }
        if ui.button(i18n::fl("settings-apply-patch")).clicked() {
            let text = state.core_patch_text.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                let result = async {
                    let patch: Value = serde_json::from_str(&text)
                        .map_err(|e| format!("invalid JSON: {e}"))?;
                    client.patch_settings(&patch).await.map_err(|e| e.to_string())?;
                    Ok(())
                }
                .await;
                crate::app::AsyncOutcome::ApplyCoreSettings(result)
            });
        }
    });
}

fn build_core_patch(state: &DetailUiState) -> Value {
    serde_json::json!({
        "ai": {
            "tasks": {
                "chat": {
                    "plugin": state.chat_plugin,
                    "model": state.chat_model,
                },
                "tts": { "plugin": state.tts_plugin },
                "stt": { "plugin": state.stt_plugin },
            }
        }
    })
}

fn show_memory(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
) {
    if !state.loaded.memory {
        state.loaded.memory = true;
        let soul_id = soul_id.to_owned();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client
                .list_memories(&soul_id, None)
                .await
                .map(|p| p.items)
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::ListMemories(result)
        });
    }
    if ui.button(i18n::fl("memory-refresh")).clicked() {
        state.loaded.memory = false;
    }
    ScrollArea::vertical().show(ui, |ui| {
        for memory in &state.memories {
            ui.group(|ui| {
                ui.label(format!("{} ({})", memory.title, memory.scope));
                ui.label(&memory.content);
                if ui.button(i18n::fl("memory-delete")).clicked() {
                    let id = memory.id.clone();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        let result = client.delete_memory(&id).await.map_err(|e| e.to_string());
                        crate::app::AsyncOutcome::DeleteMemory { id, result }
                    });
                }
            });
        }
    });
}

fn show_character(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
) {
    if !state.loaded.character {
        state.loaded.character = true;
        let soul_id = soul_id.to_owned();
        let client_for_soul = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client_for_soul
                .get_soul(&soul_id)
                .await
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::LoadSoul(result)
        });
        let client_for_list = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client_for_list
                .list_characters()
                .await
                .map(|p| p.items)
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::ListCharacters(result)
        });
    }

    if let Some(soul) = &state.soul {
        ui.label(format!("{}: {}", i18n::fl("character-soul"), soul.id));
        ui.label(format!(
            "{}: {}",
            i18n::fl("character-display"),
            soul.display_name
        ));
    }

    ui.horizontal(|ui| {
        ui.label(i18n::fl("character-body-ref"));
        ui.text_edit_singleline(&mut state.body_ref_draft);
        if ui.button(i18n::fl("character-apply-body")).clicked() {
            let body_ref = state.body_ref_draft.clone();
            let soul_id = soul_id.to_owned();
            let client = Arc::clone(&client);
            spawn_async(rt, async_results, async move {
                let result = client
                    .patch_soul_body(
                        &soul_id,
                        &ene_api::SoulPatch {
                            body_ref: Some(body_ref),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string());
                crate::app::AsyncOutcome::PatchBody(result)
            });
        }
    });

    if ui.button(i18n::fl("character-import")).clicked() {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            let path = path.display().to_string();
            let client = Arc::clone(&client);
            spawn_async(rt, async_results, async move {
                let result = client.import_character(&path).await.map_err(|e| e.to_string());
                crate::app::AsyncOutcome::ImportCharacter(result)
            });
        }
    }

    ui.separator();
    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
        for character in &state.characters {
            ui.label(format!(
                "{} v{} — {}",
                character.id, character.version, character.path
            ));
        }
    });
}

fn show_jobs(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
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
            crate::app::AsyncOutcome::ListJobs(result)
        });
    }
    if ui.button(i18n::fl("jobs-refresh")).clicked() {
        state.loaded.jobs = false;
    }

    ui.heading(i18n::fl("jobs-active"));
    ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for job in &state.jobs {
            ui.horizontal(|ui| {
                ui.label(format!("{} [{}] {}", job.title, job.status, job.id));
                if ui.button(i18n::fl("jobs-cancel")).clicked() {
                    let id = job.id.clone();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        let result = client.cancel_job(&id).await.map(|_| ()).map_err(|e| e.to_string());
                        crate::app::AsyncOutcome::CancelJob { id, result }
                    });
                }
            });
        }
    });

    ui.heading(i18n::fl("jobs-schedules"));
    ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
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
                        let result = client
                            .patch_schedule(&id, enabled)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        crate::app::AsyncOutcome::ToggleSchedule { id, enabled, result }
                    });
                }
            });
        }
    });
}

fn show_plugins(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
) {
    if !state.loaded.plugins {
        state.loaded.plugins = true;
        let client_list = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = client_list
                .list_plugins()
                .await
                .map(|p| p.items)
                .map_err(|e| e.to_string());
            crate::app::AsyncOutcome::ListPlugins(result)
        });
        let client_mcp = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = async {
                let doc = client_mcp.mcp().await.map_err(|e| e.to_string())?;
                serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
            }
            .await;
            crate::app::AsyncOutcome::LoadMcp(result)
        });
    }
    if ui.button(i18n::fl("plugins-refresh")).clicked() {
        state.loaded.plugins = false;
    }

    ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
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
                        let result = client
                            .restart_plugin(&id)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string());
                        crate::app::AsyncOutcome::RestartPlugin { id, result }
                    });
                }
            });
        }
    });

    ui.separator();
    ui.heading(i18n::fl("plugins-mcp"));
    ui.add(
        TextEdit::multiline(&mut state.mcp_json)
            .desired_width(f32::INFINITY)
            .desired_rows(10),
    );
    if ui.button(i18n::fl("plugins-mcp-save")).clicked() {
        let text = state.mcp_json.clone();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let result = async {
                let doc: ene_api::McpDocument = serde_json::from_str(&text)
                    .map_err(|e| format!("invalid MCP JSON: {e}"))?;
                client.put_mcp(&doc).await.map_err(|e| e.to_string())?;
                Ok(())
            }
            .await;
            crate::app::AsyncOutcome::SaveMcp(result)
        });
    }
}

fn spawn_async(
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<crate::app::AsyncOutcome>>>,
    task: impl std::future::Future<Output = crate::app::AsyncOutcome> + Send + 'static,
) {
    let results = Arc::clone(async_results);
    rt.spawn(async move {
        let outcome = task.await;
        results.lock().push(outcome);
    });
}
