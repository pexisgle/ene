//! Detail window: eight new-core IA sections plus a session log.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use ene_api::{
    ApiClient, ApiError, CharacterView, CreateJobRequest, JobView, MemoryCandidateDecision,
    MemoryCandidateView, MemoryJournalView, MemoryPatch, MemoryView, OccupantView,
    PluginConfigField, PluginConfigValues, PluginConfigView, PluginView, ProviderAssetView,
    ResolveMemoryCandidateRequest, ScheduleView, SoulView, ToolView,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::runtime::Handle;

mod primitives;

use crate::core::session::prepare_soul_target;
use crate::settings::DesktopSettings;
use crate::tasks::{ActivatedCharacter, AsyncOutcome};
use primitives::{EmptyState, SectionHeading, StatusCard, StatusTone, danger_hint};

use crate::i18n;

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

#[must_use]
fn caption_position_label(value: &str) -> String {
    match value {
        "top" => i18n::fl("settings-caption-position-top"),
        "left" => i18n::fl("settings-caption-position-left"),
        "right" => i18n::fl("settings-caption-position-right"),
        "bottom" => i18n::fl("settings-caption-position-bottom"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn theme_label(value: &str) -> String {
    match value {
        "system" => i18n::fl("settings-theme-system"),
        "dark" => i18n::fl("settings-theme-dark"),
        "light" => i18n::fl("settings-theme-light"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn language_value_label(value: &str) -> String {
    match value {
        "" => i18n::fl("settings-language-system"),
        "ja" => i18n::fl("settings-language-ja"),
        "en-US" => i18n::fl("settings-language-en-us"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn core_lifetime_label(value: &str) -> String {
    match value {
        "app" => i18n::fl("settings-core-lifetime-app"),
        "detached" => i18n::fl("settings-core-lifetime-detached"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn plugin_profile_label(value: &str) -> String {
    match value {
        "desktop" => i18n::fl("settings-plugins-profile-desktop"),
        "minimal" => i18n::fl("settings-plugins-profile-minimal"),
        "headless" => i18n::fl("settings-plugins-profile-headless"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn optional_task_label(value: &str) -> String {
    match value {
        "classifier" => i18n::fl("task-classifier"),
        "embedding" => i18n::fl("task-embedding"),
        "proactive" => i18n::fl("task-proactive"),
        "stt" => i18n::fl("task-stt"),
        "tts" => i18n::fl("task-tts"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn log_kind_label(kind: LogKind) -> String {
    match kind {
        LogKind::Thinking => i18n::fl("log-kind-thinking"),
        LogKind::Inner => i18n::fl("log-kind-inner"),
        LogKind::Tool => i18n::fl("log-kind-tool"),
        LogKind::Session => i18n::fl("log-kind-session"),
        LogKind::Job => i18n::fl("log-kind-job"),
        LogKind::Affect => i18n::fl("log-kind-affect"),
    }
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
                "observation",
                "privacy",
                "ocr",
                "title",
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

#[derive(Debug, Clone, Default)]
pub struct MemoryCandidateDraft {
    pub title: String,
    pub content: String,
    pub kind: String,
    pub scope: String,
}

impl From<&MemoryCandidateView> for MemoryCandidateDraft {
    fn from(candidate: &MemoryCandidateView) -> Self {
        Self {
            title: candidate.title.clone(),
            content: candidate.content.clone(),
            kind: candidate.kind.clone(),
            scope: candidate.scope.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SettingsLoadState {
    #[default]
    Unloaded,
    Loading,
    Loaded,
}

#[derive(Clone, Debug, Default)]
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
    pub tts_model: String,
    pub tts_base_url: String,
    pub tts_voice: String,
    pub tts_api_key: String,
    pub ai_tts_key_set: bool,
    pub tts_api_key_clear_pending: bool,
    pub stt_plugin: String,
    pub stt_model: String,
    pub stt_base_url: String,
    pub stt_api_key: String,
    pub ai_stt_key_set: bool,
    pub stt_api_key_clear_pending: bool,
    pub plugins_profile: String,
    pub approval_mode: String,
    pub observation_title_mode: String,
    pub observation_ocr_hint: bool,
    pub core_status: String,
    pub connections_status: String,
    pub health: String,
    pub unconfigured: Vec<String>,
    pub memories: Vec<MemoryView>,
    pub pending_memories: Vec<MemoryCandidateView>,
    pub memory_journal: Vec<MemoryJournalView>,
    pub candidate_drafts: HashMap<String, MemoryCandidateDraft>,
    pub shared_accept_armed: HashSet<String>,
    pub soul: Option<SoulView>,
    pub characters: Vec<CharacterView>,
    pub occupants: Vec<OccupantView>,
    pub body_ref_draft: String,
    pub jobs: Vec<JobView>,
    pub schedules: Vec<ScheduleView>,
    pub new_job_title: String,
    pub new_job_goal: String,
    pub new_job_inflight: bool,
    /// Job creation that failed because approval was still pending, stashed so
    /// the resolved approval can retry it once (#1178).
    pub pending_job_retry: Option<CreateJobRequest>,
    pub new_schedule_name: String,
    pub new_schedule_spec: String,
    pub new_schedule_inflight: bool,
    pub plugins: Vec<PluginView>,
    pub provider_assets_plugin: String,
    pub provider_assets: Vec<ProviderAssetView>,
    pub provider_install_jobs: HashMap<String, String>,
    pub provider_models: Vec<String>,
    pub provider_model_filter: String,
    pub mic_devices: Vec<String>,
    pub mic_devices_loaded: bool,
    pub mcp_json: String,
    pub mcp_servers: Vec<ene_api::McpServerView>,
    pub mcp_catalog: Vec<ene_api::McpCatalogEntryView>,
    pub mcp_catalog_source: String,
    pub mcp_catalog_fallback: String,
    pub mcp_selected_catalog_id: String,
    pub mcp_catalog_auth_input: String,
    pub mcp_probe_generation: u64,
    pub mcp_probe_pending: Option<String>,
    pub mcp_probe_candidate: Option<ene_api::McpCatalogEntryView>,
    pub mcp_probe_result: Option<ene_api::McpProbeResponse>,
    pub mcp_tools: Vec<ToolView>,
    pub plugin_config_id: String,
    pub plugin_config_request_id: u64,
    pub plugin_config_loading_request_id: Option<u64>,
    pub plugin_config_has: bool,
    pub plugin_config_open: bool,
    pub plugin_config_schema: String,
    pub plugin_config_values: String,
    pub plugin_config_secrets: Vec<String>,
    pub plugin_config_options_field: String,
    pub plugin_config_options: String,
    pub schema_json: String,
    pub usage_text: String,
    pub spans_text: String,
    pub save_local_pending: bool,
    pub request_chat_open: bool,
    pub restore_id: String,
    pub restore_confirm: bool,
    pub session_id: String,
    pub(crate) new_session_inflight: bool,
    pub open_spotlight: bool,
    pub spotlight_hotkey_ok: bool,
    activation_generation: u64,
    settings_state: SettingsLoadState,
    loaded: DetailLoaded,
}

#[derive(Clone, Debug, Default)]
struct DetailLoaded {
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
        self.settings_state = SettingsLoadState::Unloaded;
    }

    pub fn settings_load_failed(&mut self) {
        self.settings_state = SettingsLoadState::Unloaded;
    }

    pub(crate) fn settings_loaded(&self) -> bool {
        self.settings_state == SettingsLoadState::Loaded
    }

    pub(crate) fn begin_settings_load(&mut self) -> bool {
        if self.settings_state != SettingsLoadState::Unloaded {
            return false;
        }
        self.settings_state = SettingsLoadState::Loading;
        true
    }

    pub(crate) fn finish_settings_load(&mut self) {
        self.settings_state = SettingsLoadState::Loaded;
    }

    pub(crate) fn set_session_id(&mut self, session_id: &str) {
        session_id.clone_into(&mut self.session_id);
    }

    /// Reload core settings when Detail is reopened so external vault writes
    /// and restarts cannot leave a stale API-key banner behind.
    pub fn refresh_settings_on_open(&mut self) {
        if self.visible {
            self.invalidate_settings();
        }
    }

    /// Explicit navigation wins over the search box; otherwise search re-selects the tab every frame.
    pub fn select_tab(&mut self, tab: DetailTab) {
        self.tab = tab;
        self.search.clear();
    }

    pub fn next_activation_generation(&mut self) -> u64 {
        self.activation_generation = self.activation_generation.wrapping_add(1);
        self.activation_generation
    }

    #[must_use]
    pub fn activation_is_current(&self, generation: u64) -> bool {
        self.activation_generation == generation
    }

    pub fn next_mcp_probe_generation(&mut self) -> u64 {
        self.mcp_probe_generation = self.mcp_probe_generation.wrapping_add(1);
        self.mcp_probe_generation
    }

    #[must_use]
    pub fn mcp_probe_is_current(&self, generation: u64) -> bool {
        self.mcp_probe_generation == generation
    }

    pub fn invalidate_character(&mut self) {
        self.loaded.character = false;
        self.soul = None;
        self.occupants.clear();
        self.body_ref_draft.clear();
    }

    pub fn invalidate_memory(&mut self) {
        self.loaded.memory = false;
        self.memories.clear();
        self.pending_memories.clear();
        self.memory_journal.clear();
        self.candidate_drafts.clear();
        self.shared_accept_armed.clear();
    }

    pub(crate) fn sync_candidate_drafts(&mut self, candidates: &[MemoryCandidateView]) {
        self.candidate_drafts
            .retain(|id, _| candidates.iter().any(|candidate| candidate.id == *id));
        self.shared_accept_armed
            .retain(|id| candidates.iter().any(|candidate| candidate.id == *id));
        for candidate in candidates {
            self.candidate_drafts
                .entry(candidate.id.clone())
                .or_insert_with(|| MemoryCandidateDraft::from(candidate));
        }
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending_memories.len()
    }

    /// Resolve removes the row before the server answers; the follow-up
    /// refresh is authoritative and restores the row when the resolve failed.
    pub(crate) fn remove_candidate(&mut self, id: &str) {
        self.pending_memories.retain(|candidate| candidate.id != id);
        self.candidate_drafts.remove(id);
        self.shared_accept_armed.remove(id);
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
    state.tts_model = nested_string(effective, &["ai", "tasks", "tts", "model"]);
    state.tts_base_url = nested_string(effective, &["ai", "tasks", "tts", "base_url"]);
    state.tts_voice = nested_string(effective, &["ai", "tasks", "tts", "voice"]);
    state.ai_tts_key_set = effective
        .get("ai_tts_key_set")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    state.tts_api_key_clear_pending = false;
    state.stt_plugin = nested_string(effective, &["ai", "tasks", "stt", "plugin"]);
    state.stt_model = nested_string(effective, &["ai", "tasks", "stt", "model"]);
    state.stt_base_url = nested_string(effective, &["ai", "tasks", "stt", "base_url"]);
    state.ai_stt_key_set = effective
        .get("ai_stt_key_set")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    state.stt_api_key_clear_pending = false;
    state.plugins_profile = nested_string(effective, &["plugins", "profile"]);
    state.approval_mode = normalize_approval_mode(&nested_string(effective, &["approval", "mode"]));
    state.observation_title_mode = normalize_title_mode(&nested_string(
        effective,
        &["mind", "proactive", "world_state", "title_mode"],
    ));
    state.observation_ocr_hint =
        nested_bool(effective, &["mind", "proactive", "world_state", "ocr_hint"]);
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

fn nested_bool(value: &Value, path: &[&str]) -> bool {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return false;
        };
        current = next;
    }
    current.as_bool().unwrap_or(false)
}

#[must_use]
pub fn normalize_title_mode(raw: &str) -> String {
    match raw.to_ascii_lowercase().replace('-', "_").as_str() {
        "redacted_title" | "redacted" => "redacted_title".to_owned(),
        "full_title" | "full" => "full_title".to_owned(),
        _ => "app_only".to_owned(),
    }
}

#[must_use]
pub fn normalize_approval_mode(raw: &str) -> String {
    match raw.to_ascii_lowercase().replace('-', "_").as_str() {
        "ask_all" | "askall" => "ask_all".to_owned(),
        "ai_auto" | "aiauto" => "ai_auto".to_owned(),
        "auto" => "auto".to_owned(),
        _ => "policy".to_owned(),
    }
}

#[must_use]
pub fn mcp_args_text(args: &[String]) -> String {
    args.join("\n")
}

pub fn set_mcp_args_text(server: &mut ene_api::McpServerView, text: &str) {
    server.args = text
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .filter(|line| !line.is_empty())
        .collect();
}

pub fn load_mcp_form(state: &mut DetailUiState, json: &str) -> Result<(), String> {
    let doc: ene_api::McpDocument = serde_json::from_str(json)
        .map_err(|err| format!("{}: {err}", i18n::fl("mcp-json-invalid")))?;
    state.mcp_servers = doc.servers;
    json.clone_into(&mut state.mcp_json);
    Ok(())
}

pub fn validate_mcp_server(server: &ene_api::McpServerView) -> Result<(), String> {
    if server.id.trim().is_empty() {
        return Err(i18n::fl("mcp-id-required"));
    }
    match server.transport.as_str() {
        "stdio" | "" => {
            if server.command.as_deref().unwrap_or("").trim().is_empty() {
                return Err(i18n::fl("mcp-command-required"));
            }
        }
        "http" | "sse" | "streamable_http" | "streamable-http" => {
            if server.url.as_deref().unwrap_or("").trim().is_empty() {
                return Err(i18n::fl("mcp-url-required"));
            }
        }
        _ => return Err(i18n::fl("mcp-transport-required")),
    }
    Ok(())
}

pub fn validate_mcp_document(servers: &[ene_api::McpServerView]) -> Result<(), String> {
    for server in servers {
        validate_mcp_server(server)?;
    }
    Ok(())
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
    provider_bool(providers, plugin, "needs_key")
}

#[must_use]
pub fn plugin_is_local(plugin: &str, providers: &Value) -> bool {
    provider_bool(providers, plugin, "local")
}

/// Plugins whose clients substitute a working default when a field is blank,
/// so an empty form is functional rather than misconfigured.
#[must_use]
pub fn plugin_has_fallback(plugin: &str) -> bool {
    matches!(
        plugin,
        "provider.edge_tts" | "provider.voicevox" | "provider.openai_compat"
    )
}

fn provider_bool(providers: &Value, plugin: &str, key: &str) -> bool {
    providers
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(plugin))
        .and_then(|row| row.get(key).and_then(Value::as_bool))
        .unwrap_or(false)
}

#[must_use]
pub fn provider_display_name(id: &str) -> String {
    match id {
        "provider.gguf" => i18n::fl("provider-gguf"),
        "provider.openai_compat" => i18n::fl("provider-openai-compat"),
        "provider.anthropic" => i18n::fl("provider-anthropic"),
        "provider.elevenlabs" => i18n::fl("provider-elevenlabs"),
        "provider.voicevox" => i18n::fl("provider-voicevox"),
        "provider.edge_tts" => i18n::fl("provider-edge-tts"),
        other if other.starts_with("provider.") => other["provider.".len()..].replace('_', " "),
        other => other.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    pub id: String,
    pub label: String,
}

#[must_use]
pub fn chat_provider_choices(providers: &Value) -> Vec<ProviderChoice> {
    let mut rows: Vec<ProviderChoice> = providers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            if id.is_empty() {
                return None;
            }
            let installed = row
                .get("installed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !installed {
                return None;
            }
            let has_llm = row
                .get("seams")
                .and_then(Value::as_array)
                .is_none_or(|seams| seams.iter().any(|seam| seam.as_str() == Some("seam.llm")));
            if !has_llm {
                return None;
            }
            Some(ProviderChoice {
                id: id.to_owned(),
                label: provider_display_name(id),
            })
        })
        .collect();
    rows.sort_by(|left, right| left.label.cmp(&right.label));
    rows
}

#[must_use]
pub fn provider_choices_for_seam(providers: &Value, seam: &str) -> Vec<ProviderChoice> {
    let mut rows: Vec<ProviderChoice> = providers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_str)?;
            let installed = row
                .get("installed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let has_seam = row
                .get("seams")
                .and_then(Value::as_array)
                .is_some_and(|seams| seams.iter().any(|item| item.as_str() == Some(seam)));
            (installed && !id.is_empty() && has_seam).then(|| ProviderChoice {
                id: id.to_owned(),
                label: provider_display_name(id),
            })
        })
        .collect();
    rows.sort_by(|left, right| left.label.cmp(&right.label));
    rows
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
pub fn chat_apply_block_reason(state: &DetailUiState) -> Option<String> {
    if state.chat_plugin.is_empty() || state.chat_plugin == "echo" {
        return Some(i18n::fl("settings-chat-pick-provider"));
    }
    if state.chat_model.is_empty() {
        return Some(i18n::fl("settings-chat-pick-model"));
    }
    if plugin_needs_key(&state.chat_plugin, &state.providers)
        && !state.ai_chat_key_set
        && state.chat_api_key.is_empty()
    {
        return Some(i18n::fl("settings-chat-key-required"));
    }
    None
}

#[must_use]
pub fn home_chat_next_step(state: &DetailUiState) -> String {
    if chat_setup_gap(state) == Some(ChatSetupGap::ApiKey) {
        i18n::fl("home-next-chat-key")
    } else {
        i18n::fl("home-next-chat")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupState {
    Ready,
    NeedsSetup,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SetupCard {
    tab: DetailTab,
    state: SetupState,
}

impl SetupCard {
    fn title(self) -> String {
        self.tab.label()
    }

    fn state_label(self) -> String {
        match self.state {
            SetupState::Ready => i18n::fl("home-state-ready"),
            SetupState::NeedsSetup => i18n::fl("home-state-needs-setup"),
            SetupState::Error => i18n::fl("home-state-error"),
        }
    }

    fn detail(self, state: &DetailUiState) -> String {
        match self.tab {
            DetailTab::Conversation => home_chat_next_step(state),
            DetailTab::Voice => {
                if self.state == SetupState::Ready {
                    i18n::fl("home-voice-ready")
                } else {
                    i18n::fl("home-optional-voice")
                }
            }
            DetailTab::Companion => {
                if state.soul.is_some() {
                    i18n::fl("home-companion-ready")
                } else {
                    i18n::fl("home-next-companion")
                }
            }
            _ => String::new(),
        }
    }
}

fn home_status_cards(state: &DetailUiState) -> Vec<(DetailTab, StatusCard)> {
    setup_cards(state)
        .into_iter()
        .map(|card| {
            let tone = match card.state {
                SetupState::Ready => StatusTone::Ready,
                SetupState::NeedsSetup => StatusTone::NeedsConfig,
                SetupState::Error => StatusTone::Error,
            };
            let detail = card.detail(state);
            let status_card = StatusCard {
                state: tone,
                title: card.title(),
                summary: if detail.is_empty() {
                    card.state_label()
                } else {
                    format!("{} — {detail}", card.state_label())
                },
                action_label: Some(i18n::fl("home-open-card")),
            };
            (card.tab, status_card)
        })
        .collect()
}

fn setup_cards(state: &DetailUiState) -> Vec<SetupCard> {
    let health_error = !state.health.is_empty()
        && (state.health.contains("error") || state.health.contains("Error"));
    let companion_state = if state.soul.is_some() {
        SetupState::Ready
    } else if health_error {
        SetupState::Error
    } else {
        SetupState::NeedsSetup
    };
    vec![
        SetupCard {
            tab: DetailTab::Companion,
            state: companion_state,
        },
        SetupCard {
            tab: DetailTab::Conversation,
            state: if blocking_unconfigured(&state.unconfigured).is_empty() {
                SetupState::Ready
            } else {
                SetupState::NeedsSetup
            },
        },
        SetupCard {
            tab: DetailTab::Voice,
            state: if optional_unconfigured(&state.unconfigured).contains(&"tts")
                || optional_unconfigured(&state.unconfigured).contains(&"stt")
            {
                SetupState::NeedsSetup
            } else {
                SetupState::Ready
            },
        },
    ]
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
    state.request_chat_open = false;
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
    if matches!(state.tab, DetailTab::Home | DetailTab::Companion) {
        show_onboarding(ui, state, local_settings, client, rt, async_results);
    }
}

fn show_onboarding(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    local_settings: &mut DesktopSettings,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    if !onboarding_visible(state, local_settings) {
        return;
    }
    ui.separator();
    ui.group(|ui| {
        ui.strong(i18n::fl("onboarding-title"));
        ui.label(i18n::fl("onboarding-body"));
        ui.horizontal(|ui| {
            if ui
                .button(i18n::fl("onboarding-open-conversation"))
                .clicked()
            {
                state.select_tab(DetailTab::Conversation);
            }
            if ui.button(i18n::fl("onboarding-dismiss")).clicked() {
                local_settings.onboarding_dismissed = true;
                state.save_local_pending = true;
            }
        });
    });
}

#[must_use]
fn onboarding_visible(state: &DetailUiState, local_settings: &DesktopSettings) -> bool {
    state.settings_loaded()
        && !local_settings.onboarding_dismissed
        && chat_setup_gap(state).is_some()
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
    let cards: Vec<(DetailTab, StatusCard)> = home_status_cards(state);
    for (tab, card) in cards {
        if card.show(ui) {
            state.select_tab(tab);
        }
    }
    let optional = optional_unconfigured(&state.unconfigured);
    if !optional.is_empty() {
        let labels = optional
            .iter()
            .map(|task| optional_task_label(task))
            .collect::<Vec<_>>()
            .join(", ");
        ui.label(format!("{}: {}", i18n::fl("home-optional-tasks"), labels));
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
    ui.collapsing(i18n::fl("home-details"), |ui| {
        ui.label(format!("{}: {}", i18n::fl("home-health"), state.health));
        ui.label(format!(
            "{}: {}",
            i18n::fl("home-fibers"),
            state.plugins.len()
        ));
        ui.label(i18n::fl("home-fibers-hint"));
    });
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

async fn prepare_activation(
    client: &ApiClient,
    result: Result<CharacterView, ApiError>,
) -> Result<ActivatedCharacter, String> {
    let character = result.map_err(|err| err.to_string())?;
    let target = match character.soul_id.as_deref() {
        Some(soul_id) => Some(
            prepare_soul_target(client, soul_id)
                .await
                .map_err(|err| err.to_string())?,
        ),
        None => None,
    };
    Ok(ActivatedCharacter { character, target })
}

/// A package is the active companion when the soul it created or reused
/// matches the currently active soul, so the UI can badge it instead of
/// offering a redundant Activate action (#1177).
#[must_use]
pub fn is_character_active(
    character: &ene_api::CharacterView,
    active_soul_id: Option<&str>,
) -> bool {
    match (character.soul_id.as_deref(), active_soul_id) {
        (Some(soul_id), Some(active)) => soul_id == active,
        _ => false,
    }
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
    if ui.button(i18n::fl("tray-chat")).clicked() {
        state.request_chat_open = true;
    }
    if ui.button(i18n::fl("character-import")).clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("enechar", &["enechar", "zip", "png", "charx"])
            .pick_file()
    {
        let path = path.display().to_string();
        let generation = state.next_activation_generation();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let imported = client.import_character(&path).await;
            AsyncOutcome::ImportCharacter {
                generation,
                result: prepare_activation(&client, imported).await,
            }
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
        ui.weak(i18n::fl("character-advanced-help"));
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
        SectionHeading {
            title: i18n::fl("character-occupants"),
            help: i18n::fl("character-occupants-help"),
        }
        .show(ui);
        if renderable_occupants(&state.occupants).next().is_none() {
            EmptyState {
                title: i18n::fl("character-occupants-empty"),
                explanation: i18n::fl("character-occupants-empty-help"),
                action_label: None,
            }
            .show(ui);
        }
        for occupant in renderable_occupants(&state.occupants) {
            ui.label(format!(
                "{}\n{}  body={}  avatar={}",
                occupant.soul_id,
                occupant
                    .package_id
                    .as_deref()
                    .unwrap_or("unresolved package"),
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
    let mut activate_id = None;
    if state.characters.is_empty() {
        EmptyState {
            title: i18n::fl("character-list-empty"),
            explanation: i18n::fl("character-list-empty-help"),
            action_label: Some(i18n::fl("character-import")),
        }
        .show(ui);
    }
    egui::ScrollArea::vertical()
        .max_height(240.0)
        .show(ui, |ui| {
            for character in &state.characters {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}@{} ({}) {}",
                        character.id, character.version, character.kind, character.path
                    ));
                    if is_character_active(character, state.soul.as_ref().map(|s| s.id.as_str())) {
                        ui.weak(i18n::fl("character-active-package"));
                    } else if ui.button(i18n::fl("character-activate")).clicked() {
                        activate_id = Some(character.id.clone());
                    }
                });
            }
        });
    if let Some(id) = activate_id {
        let generation = state.next_activation_generation();
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            let activated = client.activate_character(&id).await;
            AsyncOutcome::ActivateCharacter {
                generation,
                result: prepare_activation(&client, activated).await,
            }
        });
    }
}

fn show_conversation(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ensure_settings(state, client, rt, async_results);
    SectionHeading {
        title: i18n::fl("detail-tab-conversation"),
        help: i18n::fl("settings-chat-guide"),
    }
    .show(ui);
    show_chat_provider_setup(ui, state, client, rt, async_results);
    ui.separator();
    ui.label(i18n::fl("home-optional-tasks"));
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
    ui.separator();
    show_observation_privacy(ui, state, client, rt, async_results);
}

fn show_observation_privacy(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ui.heading(i18n::fl("settings-observation"));
    ui.label(i18n::fl("settings-observation-hint"));
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-observation-title"));
        let current = normalize_title_mode(&state.observation_title_mode);
        let label = title_mode_label(&current);
        egui::ComboBox::from_id_salt("observation-title-mode")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for id in ["app_only", "redacted_title", "full_title"] {
                    if ui
                        .selectable_label(current == id, title_mode_label(id))
                        .clicked()
                    {
                        id.clone_into(&mut state.observation_title_mode);
                    }
                }
            });
    });
    ui.checkbox(
        &mut state.observation_ocr_hint,
        i18n::fl("settings-observation-ocr"),
    );
    ui.label(i18n::fl("settings-observation-ocr-hint"));
    ui.label(format!(
        "{}: {}",
        i18n::fl("settings-observation-scope"),
        observation_scope_text(state)
    ));
    ui.label(i18n::fl("settings-observation-scope-pixels"));
    if ui.button(i18n::fl("settings-observation-apply")).clicked() {
        apply_observation_patch(state, client, rt, async_results);
    }
}

fn title_mode_label(mode: &str) -> String {
    match mode {
        "redacted_title" => i18n::fl("settings-observation-redacted"),
        "full_title" => i18n::fl("settings-observation-full"),
        _ => i18n::fl("settings-observation-app-only"),
    }
}

fn observation_scope_text(state: &DetailUiState) -> String {
    let title = title_mode_label(&normalize_title_mode(&state.observation_title_mode));
    let ocr = if state.observation_ocr_hint {
        i18n::fl("settings-observation-ocr-on")
    } else {
        i18n::fl("settings-observation-ocr-off")
    };
    format!("{title}; {ocr}")
}

fn show_chat_provider_setup(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let choices = chat_provider_choices(&state.providers);
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-chat-provider"));
        let selected = if state.chat_plugin.is_empty() || state.chat_plugin == "echo" {
            i18n::fl("settings-chat-provider-none")
        } else {
            provider_display_name(&state.chat_plugin)
        };
        egui::ComboBox::from_id_salt("chat-provider")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for choice in &choices {
                    if ui
                        .selectable_label(state.chat_plugin == choice.id, &choice.label)
                        .clicked()
                    {
                        state.chat_plugin.clone_from(&choice.id);
                        state.provider_models.clear();
                    }
                }
            });
    });
    if choices.is_empty() {
        ui.label(i18n::fl("settings-chat-no-providers"));
    }
    if plugin_is_local(&state.chat_plugin, &state.providers) {
        ui.label(i18n::fl("settings-chat-local-hint"));
    } else if !state.chat_plugin.is_empty() && state.chat_plugin != "echo" {
        ui.horizontal(|ui| {
            ui.label(i18n::fl("settings-chat-base-url"));
            ui.add(
                egui::TextEdit::singleline(&mut state.chat_base_url)
                    .hint_text("https://api.example.invalid/v1"),
            );
        });
        ui.label(i18n::fl("settings-chat-base-url-hint"));
    }
    if plugin_needs_key(&state.chat_plugin, &state.providers) {
        ui.horizontal(|ui| {
            ui.label(i18n::fl("settings-chat-api-key"));
            ui.add(egui::TextEdit::singleline(&mut state.chat_api_key).password(true));
        });
        if state.ai_chat_key_set {
            ui.label(i18n::fl("settings-chat-key-set"));
        } else {
            ui.colored_label(
                egui::Color32::YELLOW,
                i18n::fl("settings-chat-key-required"),
            );
        }
    }
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-chat-model"));
        ui.text_edit_singleline(&mut state.chat_model);
    });
    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        apply_ai_patch(state, client, rt, async_results, true);
    }
    if ui.button(i18n::fl("settings-list-models")).clicked() {
        request_provider_models(state, client, rt, async_results);
    }
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
    if filtered_provider_models(&state.provider_models, &query).is_empty() {
        EmptyState {
            title: i18n::fl("settings-models-empty"),
            explanation: i18n::fl("settings-list-models-empty"),
            action_label: Some(i18n::fl("settings-list-models")),
        }
        .show(ui);
    }
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

fn request_provider_models(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let plugin = state.chat_plugin.clone();
    if plugin.is_empty() || plugin == "echo" {
        state.core_status = i18n::fl("settings-chat-pick-provider");
        return;
    }
    let base_url = state.chat_base_url.clone();
    let api_key = state.chat_api_key.clone();
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        let result = client
            .list_provider_models(&ene_api::ListProviderModelsRequest {
                plugin,
                task: "chat".to_owned(),
                base_url,
                api_key,
            })
            .await
            .map(|r| (r.models, r.error))
            .map_err(|e| e.to_string());
        AsyncOutcome::ListProviderModels(result)
    });
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
    if !state.loaded.plugins {
        state.loaded.plugins = true;
        let client_catalog = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadMcpCatalog(
                client_catalog
                    .mcp_catalog()
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
        let client_list = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListPlugins(
                client_list
                    .list_plugins()
                    .await
                    .map(|page| page.items)
                    .map_err(|err| err.to_string()),
            )
        });
    }
    if !state.mic_devices_loaded {
        state.mic_devices = crate::audio::AudioHub::list_input_device_names();
        state.mic_devices_loaded = true;
    }
    SectionHeading {
        title: i18n::fl("detail-tab-voice"),
        help: i18n::fl("settings-voice-guide"),
    }
    .show(ui);
    let providers = state.providers.clone();
    show_voice_task(
        ui,
        &providers,
        VoiceTaskForm {
            id: "tts",
            label: i18n::fl("settings-tts-plugin"),
            seam: "seam.tts",
            plugin: &mut state.tts_plugin,
            model: &mut state.tts_model,
            base_url: &mut state.tts_base_url,
            voice: Some(&mut state.tts_voice),
            api_key: &mut state.tts_api_key,
            key_set: &mut state.ai_tts_key_set,
            clear_pending: &mut state.tts_api_key_clear_pending,
        },
    );
    show_voice_provider_config_button(
        ui,
        state,
        client,
        rt,
        async_results,
        "tts",
        &state.tts_plugin.clone(),
    );
    ui.separator();
    show_voice_task(
        ui,
        &providers,
        VoiceTaskForm {
            id: "stt",
            label: i18n::fl("settings-stt-plugin"),
            seam: "seam.stt",
            plugin: &mut state.stt_plugin,
            model: &mut state.stt_model,
            base_url: &mut state.stt_base_url,
            voice: None,
            api_key: &mut state.stt_api_key,
            key_set: &mut state.ai_stt_key_set,
            clear_pending: &mut state.stt_api_key_clear_pending,
        },
    );
    show_voice_provider_config_button(
        ui,
        state,
        client,
        rt,
        async_results,
        "stt",
        &state.stt_plugin.clone(),
    );
    show_plugin_config(ui, state, client, rt, async_results);
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-mic-device"));
        let stored_missing = !local_settings.mic_device.is_empty()
            && !state
                .mic_devices
                .iter()
                .any(|name| name == &local_settings.mic_device);
        let selected = if local_settings.mic_device.is_empty() {
            i18n::fl("settings-mic-system-default")
        } else if stored_missing {
            format!(
                "{}: {}",
                i18n::fl("settings-mic-missing"),
                local_settings.mic_device
            )
        } else {
            local_settings.mic_device.clone()
        };
        egui::ComboBox::from_id_salt("stage-mic-device")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut local_settings.mic_device,
                    String::new(),
                    i18n::fl("settings-mic-system-default"),
                );
                for name in &state.mic_devices {
                    ui.selectable_value(&mut local_settings.mic_device, name.clone(), name);
                }
            });
        if ui.button(i18n::fl("settings-mic-refresh")).clicked() {
            state.mic_devices_loaded = false;
        }
    });
    if state.mic_devices.is_empty() {
        ui.colored_label(egui::Color32::YELLOW, i18n::fl("settings-mic-unavailable"));
    } else if !local_settings.mic_device.is_empty()
        && !state
            .mic_devices
            .iter()
            .any(|name| name == &local_settings.mic_device)
    {
        ui.colored_label(egui::Color32::YELLOW, i18n::fl("settings-mic-missing-hint"));
    } else {
        ui.label(i18n::fl("settings-mic-ready"));
    }
    ui.checkbox(
        &mut local_settings.caption_enabled,
        i18n::fl("settings-captions"),
    );
    ui.checkbox(
        &mut local_settings.spotlight_enabled,
        i18n::fl("settings-spotlight"),
    );
    let hotkey_fallback = !state.spotlight_hotkey_ok && local_settings.spotlight_enabled;
    if hotkey_fallback {
        ui.colored_label(
            egui::Color32::YELLOW,
            i18n::fl("settings-spotlight-hotkey-fallback"),
        );
    }
    if ui
        .button(egui::RichText::new(i18n::fl("settings-open-spotlight")).strong())
        .clicked()
    {
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
            .selected_text(caption_position_label(current))
            .show_ui(ui, |ui| {
                for position in crate::surface::caption::POSITIONS {
                    ui.selectable_value(
                        &mut local_settings.caption_position,
                        position.to_owned(),
                        caption_position_label(position),
                    );
                }
            });
    });
    ui.checkbox(
        &mut local_settings.caption_pinned,
        i18n::fl("settings-caption-pin"),
    );
    if ui.button(i18n::fl("settings-voice-apply")).clicked() {
        apply_voice_patch(state, client, rt, async_results);
    }
    if ui.button(i18n::fl("settings-save-local")).clicked() {
        state.save_local_pending = true;
    }
}

struct VoiceTaskForm<'a> {
    id: &'static str,
    label: String,
    seam: &'static str,
    plugin: &'a mut String,
    model: &'a mut String,
    base_url: &'a mut String,
    voice: Option<&'a mut String>,
    api_key: &'a mut String,
    key_set: &'a mut bool,
    clear_pending: &'a mut bool,
}

fn show_voice_task(ui: &mut egui::Ui, providers: &Value, mut form: VoiceTaskForm<'_>) {
    ui.heading(&form.label);
    let choices = provider_choices_for_seam(providers, form.seam);
    let selected = if form.plugin.is_empty() || form.plugin == "echo" {
        i18n::fl("settings-voice-provider-none")
    } else {
        provider_display_name(form.plugin)
    };
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-voice-provider"));
        egui::ComboBox::from_id_salt(("voice-provider", form.id))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for choice in &choices {
                    if ui
                        .selectable_label(*form.plugin == choice.id, &choice.label)
                        .clicked()
                    {
                        form.plugin.clone_from(&choice.id);
                        form.model.clear();
                        form.base_url.clear();
                        if let Some(voice) = form.voice.as_deref_mut() {
                            voice.clear();
                        }
                        form.api_key.clear();
                        *form.key_set = false;
                        *form.clear_pending = true;
                    }
                }
            });
    });
    if choices.is_empty() {
        ui.label(i18n::fl("settings-voice-provider-empty"));
    }
    if plugin_is_local(form.plugin, providers) {
        ui.label(i18n::fl("settings-voice-local-hint"));
    } else if !form.plugin.is_empty() && form.plugin != "echo" {
        task_row(ui, i18n::fl("settings-voice-base-url"), form.base_url);
    }
    // Empty fields are valid for plugins with built-in fallbacks; leaving them
    // blank-looking under a green Ready card reads as a lost save (#1177).
    if plugin_has_fallback(form.plugin) {
        let needs_base_note =
            !plugin_is_local(form.plugin, providers) && form.base_url.trim().is_empty();
        let needs_model_note = form.model.trim().is_empty();
        let needs_voice_note = form
            .voice
            .as_deref()
            .is_some_and(|value| value.trim().is_empty());
        if needs_base_note || needs_model_note || needs_voice_note {
            ui.label(i18n::fl("settings-voice-fallback-note"));
        }
    }
    task_row(ui, i18n::fl("settings-voice-model"), form.model);
    if let Some(voice) = form.voice {
        task_row(ui, i18n::fl("settings-voice-voice"), voice);
    }
    if plugin_needs_key(form.plugin, providers) {
        ui.horizontal(|ui| {
            ui.label(i18n::fl("settings-voice-api-key"));
            let changed = ui
                .add(egui::TextEdit::singleline(form.api_key).password(true))
                .changed();
            if changed && !form.api_key.is_empty() {
                *form.clear_pending = false;
            }
        });
        if *form.key_set {
            ui.label(i18n::fl("settings-voice-key-set"));
            if ui.button(i18n::fl("settings-voice-clear-key")).clicked() {
                *form.key_set = false;
                *form.clear_pending = true;
            }
        } else {
            ui.colored_label(
                egui::Color32::YELLOW,
                i18n::fl("settings-voice-key-required"),
            );
        }
    }
    let ready = !form.plugin.is_empty()
        && form.plugin != "echo"
        && (!plugin_needs_key(form.plugin, providers) || *form.key_set || !form.api_key.is_empty());
    StatusCard {
        state: if ready {
            StatusTone::Ready
        } else if choices.is_empty() {
            StatusTone::Error
        } else {
            StatusTone::NeedsConfig
        },
        title: form.label.clone(),
        summary: if ready {
            i18n::fl("settings-voice-ready")
        } else if choices.is_empty() {
            i18n::fl("settings-voice-provider-empty")
        } else {
            i18n::fl("settings-voice-not-configured")
        },
        action_label: None,
    }
    .show(ui);
}

fn show_voice_provider_config_button(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
    task: &str,
    plugin: &str,
) {
    if plugin.is_empty() || plugin == "echo" {
        return;
    }
    let row_id = format!("ai.tasks.{task}");
    let Some(row_id) = state
        .plugins
        .iter()
        .find(|row| row.row_id == row_id && row.plugin == plugin)
        .map(|row| row.row_id.clone())
    else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-voice-provider-config"));
        if ui
            .button(i18n::fl("settings-voice-provider-configure"))
            .clicked()
        {
            let id = row_id.clone();
            let request_id = begin_plugin_config_load(state, &id);
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::LoadPluginConfig {
                    request_id,
                    id: id.clone(),
                    result: client
                        .plugin_config(&id)
                        .await
                        .map_err(|err| err.to_string()),
                }
            });
        }
    });
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
            AsyncOutcome::ListMemories {
                soul_id: soul_id_mem.clone(),
                result: client_m
                    .list_memories(&soul_id_mem, None)
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            }
        });
        let soul_id_pending = soul_id.to_owned();
        let client_p = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListPendingMemories {
                soul_id: soul_id_pending.clone(),
                result: client_p
                    .list_pending_memories(&soul_id_pending)
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            }
        });
        let soul_id_journal = soul_id.to_owned();
        let client_journal = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ListMemoryJournal {
                soul_id: soul_id_journal.clone(),
                result: client_journal
                    .list_memory_journal(&soul_id_journal)
                    .await
                    .map(|p| p.items)
                    .map_err(|e| e.to_string()),
            }
        });
    }
    if ui.button(i18n::fl("memory-refresh")).clicked() {
        state.loaded.memory = false;
    }
    let pending = state.pending_memories.clone();
    SectionHeading {
        title: i18n::fl("memory-candidates"),
        help: i18n::fl("memory-candidates-help"),
    }
    .show(ui);
    ui.weak(format!("{}", state.pending_count()));
    if pending.is_empty() {
        EmptyState {
            title: i18n::fl("memory-pending-empty"),
            explanation: i18n::fl("memory-pending-empty-help"),
            action_label: None,
        }
        .show(ui);
    }
    for candidate in pending {
        let draft = state
            .candidate_drafts
            .entry(candidate.id.clone())
            .or_insert_with(|| MemoryCandidateDraft::from(&candidate));
        let shared = candidate.scope == "shared" || draft.scope == "shared";
        let armed = state.shared_accept_armed.contains(&candidate.id);
        let mut accept = false;
        let mut reject = false;
        let card = if shared {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.5, egui::Color32::YELLOW))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(6.0)
        } else {
            egui::Frame::group(ui.style())
        };
        card.show(ui, |ui| {
            if shared {
                ui.colored_label(egui::Color32::YELLOW, i18n::fl("memory-shared-badge"));
            }
            ui.horizontal(|ui| {
                ui.label(i18n::fl("memory-title"));
                ui.text_edit_singleline(&mut draft.title);
            });
            ui.label(i18n::fl("memory-content"));
            ui.add(
                egui::TextEdit::multiline(&mut draft.content)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                ui.label(i18n::fl("memory-kind"));
                egui::ComboBox::from_id_salt(format!("memory-kind-{}", candidate.id))
                    .selected_text(memory_kind_label(&draft.kind))
                    .show_ui(ui, |ui| {
                        for kind in [
                            "episodic",
                            "semantic",
                            "user_profile",
                            "preference",
                            "commitment",
                        ] {
                            ui.selectable_value(
                                &mut draft.kind,
                                kind.to_owned(),
                                memory_kind_label(kind),
                            );
                        }
                    });
                ui.label(i18n::fl("memory-scope"));
                egui::ComboBox::from_id_salt(format!("memory-scope-{}", candidate.id))
                    .selected_text(memory_scope_label(&draft.scope))
                    .show_ui(ui, |ui| {
                        for scope in ["private", "shared"] {
                            ui.selectable_value(
                                &mut draft.scope,
                                scope.to_owned(),
                                memory_scope_label(scope),
                            );
                        }
                    });
            });
            ui.label(format!(
                "{} · {} · {}: {:.0}%",
                memory_kind_label(&candidate.kind),
                memory_scope_label(&candidate.scope),
                i18n::fl("memory-confidence"),
                candidate.confidence * 100.0
            ));
            if candidate.sensitive {
                ui.colored_label(egui::Color32::YELLOW, i18n::fl("memory-sensitive"));
            }
            if shared {
                ui.colored_label(egui::Color32::YELLOW, i18n::fl("memory-shared-warning"));
            }
            ui.horizontal(|ui| {
                if shared && !armed {
                    if ui.button(i18n::fl("memory-accept-confirm")).clicked() {
                        state.shared_accept_armed.insert(candidate.id.clone());
                    }
                } else {
                    if ui.button(i18n::fl("memory-accept")).clicked() {
                        accept = true;
                    }
                    if shared && ui.button(i18n::fl("memory-cancel")).clicked() {
                        state.shared_accept_armed.remove(&candidate.id);
                    }
                }
                if ui.button(i18n::fl("memory-reject")).clicked() {
                    reject = true;
                }
            });
        });
        if accept || reject {
            let payload = if accept {
                ResolveMemoryCandidateRequest {
                    decision: MemoryCandidateDecision::Accept,
                    title: Some(draft.title.clone()),
                    content: Some(draft.content.clone()),
                    kind: Some(draft.kind.clone()),
                    scope: Some(draft.scope.clone()),
                }
            } else {
                ResolveMemoryCandidateRequest {
                    decision: MemoryCandidateDecision::Reject,
                    title: None,
                    content: None,
                    kind: None,
                    scope: None,
                }
            };
            state.remove_candidate(&candidate.id);
            resolve_memory(
                &candidate.id,
                payload,
                &candidate,
                soul_id,
                client,
                rt,
                async_results,
            );
        }
    }
    ui.heading(i18n::fl("memory-history"));
    let decisions: Vec<&MemoryJournalView> = state
        .memory_journal
        .iter()
        .filter(|entry| {
            matches!(
                entry.action.as_str(),
                "candidate_accepted" | "candidate_rejected"
            )
        })
        .take(20)
        .collect();
    if decisions.is_empty() {
        ui.label(i18n::fl("memory-history-empty"));
    }
    for entry in decisions {
        let action = memory_journal_action_label(&entry.action);
        let title = entry
            .payload
            .get("accepted")
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .or_else(|| {
                entry
                    .payload
                    .get("original")
                    .and_then(|value| value.get("title"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        ui.label(format!("{action} — {title} ({})", entry.ts));
    }
    ui.heading(i18n::fl("memory-commitments"));
    let commitments: Vec<&MemoryView> = state
        .memories
        .iter()
        .filter(|memory| memory.kind == "commitment")
        .collect();
    if commitments.is_empty() {
        ui.label(i18n::fl("memory-commitments-empty"));
    }
    for memory in commitments {
        ui.group(|ui| {
            ui.label(format!(
                "{} [{}] ({})",
                memory.title,
                memory_kind_label(&memory.kind),
                memory_scope_label(&memory.scope)
            ));
            ui.label(&memory.content);
            if let Some(due) = memory.expires_at.as_deref() {
                ui.label(format!("{}: {due}", i18n::fl("memory-due")));
            }
            if let Some(schedule_id) = memory.schedule_id.as_deref() {
                ui.label(format!("{}: {schedule_id}", i18n::fl("memory-schedule")));
            }
            ui.horizontal(|ui| {
                if ui.button(i18n::fl("memory-complete")).clicked() {
                    let id = memory.id.clone();
                    let soul_id = soul_id.to_owned();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::CompleteMemory {
                            soul_id,
                            id: id.clone(),
                            result: client
                                .patch_memory(
                                    &id,
                                    &MemoryPatch {
                                        completed: Some(true),
                                        ..MemoryPatch::default()
                                    },
                                )
                                .await
                                .map(|_| ())
                                .map_err(|e| e.to_string()),
                        }
                    });
                }
                if ui.button(i18n::fl("memory-delete")).clicked() {
                    let id = memory.id.clone();
                    let soul_id = soul_id.to_owned();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::DeleteMemory {
                            soul_id,
                            id: id.clone(),
                            result: client.delete_memory(&id).await.map_err(|e| e.to_string()),
                        }
                    });
                }
            });
        });
    }
    ui.heading(i18n::fl("detail-tab-memory"));
    let others: Vec<&MemoryView> = state
        .memories
        .iter()
        .filter(|memory| memory.kind != "commitment")
        .collect();
    if others.is_empty() && state.memories.is_empty() {
        EmptyState {
            title: i18n::fl("memory-empty"),
            explanation: i18n::fl("memory-empty-help"),
            action_label: None,
        }
        .show(ui);
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for memory in others {
            ui.group(|ui| {
                ui.label(format!(
                    "{} [{}] ({})",
                    memory.title,
                    memory_kind_label(&memory.kind),
                    memory_scope_label(&memory.scope)
                ));
                ui.label(&memory.content);
                if ui.button(i18n::fl("memory-delete")).clicked() {
                    let id = memory.id.clone();
                    let soul_id = soul_id.to_owned();
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::DeleteMemory {
                            soul_id,
                            id: id.clone(),
                            result: client.delete_memory(&id).await.map_err(|e| e.to_string()),
                        }
                    });
                }
            });
        }
    });
}

#[must_use]
fn memory_kind_label(value: &str) -> String {
    match value {
        "episodic" => i18n::fl("memory-kind-episodic"),
        "semantic" => i18n::fl("memory-kind-semantic"),
        "user_profile" => i18n::fl("memory-kind-user-profile"),
        "preference" => i18n::fl("memory-kind-preference"),
        "commitment" => i18n::fl("memory-kind-commitment"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn memory_scope_label(value: &str) -> String {
    match value {
        "private" => i18n::fl("memory-scope-private"),
        "shared" => i18n::fl("memory-scope-shared"),
        _ => value.to_owned(),
    }
}

#[must_use]
fn memory_journal_action_label(value: &str) -> String {
    match value {
        "candidate_accepted" => i18n::fl("memory-history-accepted"),
        "candidate_rejected" => i18n::fl("memory-history-rejected"),
        _ => value.to_owned(),
    }
}

fn resolve_memory(
    id: &str,
    request: ResolveMemoryCandidateRequest,
    original: &MemoryCandidateView,
    soul_id: &str,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let id = id.to_owned();
    let soul_id = soul_id.to_owned();
    let original = original.clone();
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        let result = client
            .resolve_memory_candidate(&id, &request)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        if result.is_ok() {
            AsyncOutcome::ResolveMemory {
                soul_id,
                id: id.clone(),
                result,
            }
        } else {
            AsyncOutcome::ResolveMemoryFailedKeepCandidate {
                soul_id,
                original,
                result,
            }
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
        if !state.new_session_inflight && ui.button(i18n::fl("chat-new-session")).clicked() {
            state.new_session_inflight = true;
            let session_id = state.session_id.clone();
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::NewSession(
                    client
                        .split_session(&session_id)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
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
    ui.heading(i18n::fl("jobs-new"));
    ui.label(i18n::fl("jobs-new-hint"));
    ui.add(
        egui::TextEdit::singleline(&mut state.new_job_title).hint_text(i18n::fl("jobs-new-title")),
    );
    ui.add(
        egui::TextEdit::multiline(&mut state.new_job_goal)
            .desired_rows(3)
            .hint_text(i18n::fl("jobs-new-goal")),
    );
    let can_create = !state.new_job_inflight && !state.new_job_goal.trim().is_empty();
    // A stashed request means the last click was blocked by an approval ask;
    // relabeling keeps the retry visible instead of the button silently
    // duplicating a job the user believes was already created (#1178).
    let create_label = if state.pending_job_retry.is_some() {
        i18n::fl("jobs-create-retry")
    } else {
        i18n::fl("jobs-create")
    };
    if ui
        .add_enabled(can_create, egui::Button::new(create_label))
        .clicked()
    {
        state.new_job_inflight = true;
        let request = CreateJobRequest {
            soul_id: soul_id.to_owned(),
            goal: state.new_job_goal.trim().to_owned(),
            title: (!state.new_job_title.trim().is_empty())
                .then(|| state.new_job_title.trim().to_owned()),
        };
        state.pending_job_retry = Some(request.clone());
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::CreateJob(client.create_job(&request).await.map_err(|e| e.to_string()))
        });
    }
    ui.heading(i18n::fl("jobs-active"));
    let active_jobs = active_jobs(&state.jobs);
    if active_jobs.is_empty() {
        EmptyState {
            title: i18n::fl("jobs-empty"),
            explanation: i18n::fl("jobs-empty-help"),
            action_label: None,
        }
        .show(ui);
    }
    for job in active_jobs {
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
    let recent_jobs = recent_jobs(&state.jobs);
    if !recent_jobs.is_empty() {
        ui.heading(i18n::fl("jobs-recent"));
        for job in recent_jobs {
            ui.label(format!("{} [{}] {}", job.title, job.status, job.id));
        }
    }
    ui.heading(i18n::fl("jobs-schedules"));
    ui.add(
        egui::TextEdit::singleline(&mut state.new_schedule_name)
            .hint_text(i18n::fl("schedule-new-name")),
    );
    ui.add(
        egui::TextEdit::singleline(&mut state.new_schedule_spec)
            .hint_text(i18n::fl("schedule-new-spec")),
    );
    let can_create = !state.new_schedule_inflight
        && !state.new_schedule_name.trim().is_empty()
        && !state.new_schedule_spec.trim().is_empty();
    if ui
        .add_enabled(can_create, egui::Button::new(i18n::fl("schedule-create")))
        .clicked()
    {
        state.new_schedule_inflight = true;
        let request = ene_api::CreateScheduleRequest {
            soul_id: soul_id.to_owned(),
            name: state.new_schedule_name.trim().to_owned(),
            spec: state.new_schedule_spec.trim().to_owned(),
            timezone: "UTC".to_owned(),
            action: "remind".to_owned(),
            action_ref: None,
            important: false,
        };
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::CreateSchedule(
                client
                    .create_schedule(&request)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }
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

fn active_jobs(jobs: &[JobView]) -> Vec<&JobView> {
    jobs.iter()
        .filter(|job| matches!(job.status.as_str(), "created" | "queued" | "running"))
        .collect()
}

fn recent_jobs(jobs: &[JobView]) -> Vec<&JobView> {
    jobs.iter()
        .filter(|job| {
            matches!(
                job.status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            )
        })
        .take(5)
        .collect()
}

fn renderable_occupants(occupants: &[OccupantView]) -> impl Iterator<Item = &OccupantView> {
    occupants
        .iter()
        .filter(|occupant| occupant.package_id.is_some() || occupant.avatar_path.is_some())
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
        let client_catalog = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadMcpCatalog(
                client_catalog
                    .mcp_catalog()
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
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
        let client_tools = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadTools(
                client_tools
                    .list_tools()
                    .await
                    .map(|t| t.items)
                    .map_err(|e| e.to_string()),
            )
        });
        let client_catalog = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::LoadMcpCatalog(
                client_catalog
                    .mcp_catalog()
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }
    SectionHeading {
        title: i18n::fl("detail-tab-connections"),
        help: i18n::fl("connections-help"),
    }
    .show(ui);
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-plugins-profile"));
        for profile in ["desktop", "minimal", "headless"] {
            if ui
                .selectable_label(
                    state.plugins_profile == profile,
                    plugin_profile_label(profile),
                )
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
    if state.plugins.is_empty() {
        EmptyState {
            title: i18n::fl("connections-empty"),
            explanation: i18n::fl("connections-empty-help"),
            action_label: None,
        }
        .show(ui);
    } else {
        for plugin in state.plugins.clone() {
            ui.horizontal(|ui| {
                let status = connection_status_label(&plugin);
                ui.label(format!("{} ({})", plugin.plugin, status));
                ui.push_id(&plugin.row_id, |ui| {
                    ui.collapsing(plugin.plugin.clone(), |ui| {
                        ui.label(&plugin.row_id);
                    });
                });
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
                if ui.button(i18n::fl("plugins-config")).clicked() {
                    let id = plugin.row_id.clone();
                    let request_id = begin_plugin_config_load(state, &id);
                    let client = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::LoadPluginConfig {
                            request_id,
                            id: id.clone(),
                            result: client.plugin_config(&id).await.map_err(|e| e.to_string()),
                        }
                    });
                }
            });
        }
    }
    show_plugin_config(ui, state, client, rt, async_results);
    show_provider_assets(ui, state, client, rt, async_results);
    if let Some(pending) = state.mcp_probe_pending.clone() {
        ui.horizontal(|ui| {
            ui.label(i18n::format(
                "connections-mcp-probing",
                &[("id", pending.as_str())],
            ));
            if ui
                .button(i18n::fl("connections-mcp-probe-cancel"))
                .clicked()
            {
                state.next_mcp_probe_generation();
                state.mcp_probe_pending = None;
                state.mcp_probe_result = None;
            }
        });
    }
    if let Some(candidate) = state.mcp_probe_candidate.clone() {
        ui.group(|ui| {
            ui.heading(i18n::fl("connections-mcp-preview-title"));
            let Some(result) = state.mcp_probe_result.clone() else {
                return;
            };
            if let Some(err) = result.error {
                let auth_required = candidate.auth != ene_api::McpCatalogAuthView::None;
                if auth_required && result.stored_auth {
                    ui.label(i18n::fl("connections-mcp-preview-stored-auth"));
                } else {
                    ui.label(if auth_required {
                        i18n::fl("connections-status-auth-required")
                    } else {
                        i18n::fl("connections-status-unhealthy-no-error")
                    });
                }
                ui.label(err);
                // Auth-required remotes can still be added disabled; the
                // secret is then provided through plugin config.
                let candidate_exists = state.mcp_probe_candidate.is_some();
                if candidate_exists && ui.button(i18n::fl("connections-mcp-preview-add")).clicked()
                {
                    add_probed_server(state, &candidate);
                }
            } else {
                if ui.button(i18n::fl("connections-mcp-preview-add")).clicked() {
                    add_probed_server(state, &candidate);
                }
                if result.tools.is_empty() {
                    ui.label(i18n::fl("connections-mcp-preview-empty"));
                }
                for tool in &result.tools {
                    ui.horizontal(|ui| {
                        ui.label(&tool.name);
                        ui.small(&tool.description);
                    });
                    if !tool.side_effects.is_empty() {
                        ui.label(i18n::format(
                            "connections-mcp-tools-side-effects",
                            &[("effects", tool.side_effects.join(", ").as_str())],
                        ));
                    }
                }
            }
        });
    }
    show_mcp_tools(ui, state);
    ui.separator();
    show_mcp_form(ui, state, client, rt, async_results);
}

/// Persist the exact probed catalog entry as a disabled row. Enabling stays a
/// separate user action after the preview (and any secret setup) succeeds.
fn add_probed_server(state: &mut DetailUiState, candidate: &ene_api::McpCatalogEntryView) {
    if !state
        .mcp_servers
        .iter()
        .any(|server| server.id == candidate.id)
    {
        state.mcp_servers.push(ene_api::McpServerView {
            id: candidate.id.clone(),
            transport: candidate.transport.clone(),
            command: candidate.command.clone(),
            args: candidate.args.clone(),
            url: candidate.url.clone(),
            enabled: false,
        });
    }
    state.mcp_probe_candidate = None;
    state.mcp_probe_result = None;
}

fn connection_status_label(plugin: &PluginView) -> String {
    if let Some(error) = &plugin.last_error {
        let lowered = error.to_lowercase();
        if ["401", "unauthorized", "auth"]
            .iter()
            .any(|needle| lowered.contains(needle))
        {
            return i18n::fl("connections-status-auth-required");
        }
        return i18n::format(
            "connections-status-unhealthy-error",
            &[("error", error.as_str())],
        );
    }
    match plugin.state.as_str() {
        "active" => i18n::fl("connections-status-active"),
        "loading" | "waiting" => i18n::fl("connections-status-connecting"),
        "failed" => i18n::fl("connections-status-unhealthy-no-error"),
        _ => i18n::fl("connections-status-disabled"),
    }
}

fn show_mcp_tools(ui: &mut egui::Ui, state: &DetailUiState) {
    let tools: Vec<&ToolView> = state
        .mcp_tools
        .iter()
        .filter(|tool| tool.name.starts_with("mcp:"))
        .collect();
    if tools.is_empty() {
        return;
    }
    ui.separator();
    ui.heading(i18n::fl("connections-mcp-tools-title"));
    for tool in tools {
        ui.horizontal(|ui| {
            ui.label(&tool.name);
            ui.small(&tool.description);
        });
        if !tool.side_effects.is_empty() {
            ui.label(i18n::format(
                "connections-mcp-tools-side-effects",
                &[("effects", tool.side_effects.join(", ").as_str())],
            ));
        }
    }
}

fn show_plugin_config(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if state.plugin_config_id.is_empty() || !state.plugin_config_open {
        return;
    }
    if plugin_config_is_loading(state) {
        ui.spinner();
        return;
    }
    ui.separator();
    ui.heading(i18n::fl("plugins-config-heading"));
    ui.label(format!(
        "{} — {}",
        state.plugin_config_id,
        if state.plugin_config_has {
            i18n::fl("plugins-config-declared")
        } else {
            i18n::fl("plugins-config-none")
        }
    ));
    if !state.plugin_config_secrets.is_empty() {
        ui.weak(format!(
            "{} {}",
            i18n::fl("plugins-config-secrets"),
            state.plugin_config_secrets.join(", ")
        ));
    }
    if editable_schema_fields(&state.plugin_config_schema) {
        ui.collapsing(i18n::fl("plugins-config-schema"), |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut state.plugin_config_schema)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6),
            );
        });
        ui.label(i18n::fl("plugins-config-values"));
        ui.add(
            egui::TextEdit::multiline(&mut state.plugin_config_values)
                .desired_width(f32::INFINITY)
                .desired_rows(6),
        );
        ui.horizontal(|ui| {
            let values_empty = plugin_config_values_empty(&state.plugin_config_values);
            let values_valid = plugin_config_values_valid(&state.plugin_config_values);
            if ui
                .add_enabled(
                    values_valid && !values_empty,
                    egui::Button::new(i18n::fl("plugins-config-validate")),
                )
                .clicked()
            {
                spawn_plugin_config_values(
                    state,
                    client,
                    rt,
                    async_results,
                    PluginConfigAction::Validate,
                );
            }
            if ui
                .add_enabled(
                    values_valid && !values_empty,
                    egui::Button::new(i18n::fl("plugins-config-apply")),
                )
                .clicked()
            {
                spawn_plugin_config_values(
                    state,
                    client,
                    rt,
                    async_results,
                    PluginConfigAction::Apply,
                );
            }
            if values_empty {
                ui.weak(i18n::fl("plugins-config-empty-apply"));
            } else if !values_valid {
                ui.weak(i18n::fl("plugins-config-invalid-json"));
            }
        });
        ui.horizontal(|ui| {
            ui.label(i18n::fl("plugins-config-options-field"));
            ui.text_edit_singleline(&mut state.plugin_config_options_field);
            if ui.button(i18n::fl("plugins-config-options")).clicked() {
                let id = state.plugin_config_id.clone();
                let field = state.plugin_config_options_field.clone();
                let client = Arc::clone(client);
                spawn_async(rt, async_results, async move {
                    AsyncOutcome::PluginConfigOptions(
                        client
                            .plugin_config_options(&id, &PluginConfigField { field })
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
        });
        if !state.plugin_config_options.is_empty() {
            ui.weak(&state.plugin_config_options);
        }
    } else {
        ui.label(i18n::fl("plugins-config-not-editable"));
    }
    if ui.button(i18n::fl("plugins-config-close")).clicked() {
        state.plugin_config_open = false;
    }
}

#[must_use]
pub fn editable_schema_fields(schema: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(schema)
        .ok()
        .and_then(|schema| {
            schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| !properties.is_empty())
        })
        .unwrap_or(false)
}

enum PluginConfigAction {
    Validate,
    Apply,
}

pub fn begin_plugin_config_load(state: &mut DetailUiState, id: &str) -> u64 {
    state.plugin_config_request_id = state.plugin_config_request_id.wrapping_add(1);
    state.plugin_config_loading_request_id = Some(state.plugin_config_request_id);
    id.clone_into(&mut state.plugin_config_id);
    state.plugin_config_has = false;
    state.plugin_config_schema.clear();
    state.plugin_config_values.clear();
    state.plugin_config_secrets.clear();
    state.plugin_config_options_field.clear();
    state.plugin_config_options.clear();
    state.connections_status.clear();
    state.plugin_config_open = true;
    state.plugin_config_request_id
}

#[must_use]
pub fn plugin_config_is_loading(state: &DetailUiState) -> bool {
    state.plugin_config_loading_request_id == Some(state.plugin_config_request_id)
}

#[must_use]
pub fn plugin_config_values_empty(values: &str) -> bool {
    let trimmed = values.trim();
    trimmed.is_empty()
        || serde_json::from_str::<serde_json::Value>(trimmed)
            .is_ok_and(|value| value == serde_json::json!({}))
}

#[must_use]
pub fn plugin_config_values_valid(values: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(values.trim()).is_ok()
}

#[must_use]
pub fn plugin_config_load_is_current(
    state: &DetailUiState,
    requested_id: &str,
    request_id: u64,
) -> bool {
    plugin_config_request_is_current(&state.plugin_config_id, requested_id)
        && state.plugin_config_loading_request_id == Some(request_id)
}

fn spawn_plugin_config_values(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
    action: PluginConfigAction,
) {
    let values = match serde_json::from_str::<serde_json::Value>(&state.plugin_config_values) {
        Ok(values) => values,
        Err(err) => {
            state.connections_status = err.to_string();
            return;
        }
    };
    let id = state.plugin_config_id.clone();
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        let body = PluginConfigValues { values };
        match action {
            PluginConfigAction::Validate => AsyncOutcome::ValidatePluginConfig(
                client
                    .validate_plugin_config(&id, &body)
                    .await
                    .map_err(|e| e.to_string()),
            ),
            PluginConfigAction::Apply => AsyncOutcome::ApplyPluginConfig(
                client
                    .apply_plugin_config(&id, &body)
                    .await
                    .map_err(|e| e.to_string()),
            ),
        }
    });
}

pub fn apply_plugin_config_view(state: &mut DetailUiState, view: PluginConfigView) {
    view.row_id.clone_into(&mut state.plugin_config_id);
    state.plugin_config_has = view.has_config;
    state.plugin_config_secrets = view.secret_keys;
    state.plugin_config_schema =
        serde_json::to_string_pretty(&view.schema).unwrap_or_else(|_| view.schema.to_string());
    state.plugin_config_values =
        serde_json::to_string_pretty(&view.values).unwrap_or_else(|_| view.values.to_string());
}

#[must_use]
pub fn plugin_config_request_is_current(current_id: &str, requested_id: &str) -> bool {
    !current_id.is_empty() && current_id == requested_id
}

pub fn plugin_config_status(view: &ene_api::PluginConfigValidateView) -> String {
    if view.ok {
        if view.restart_required {
            i18n::fl("plugins-config-restart")
        } else {
            i18n::fl("plugins-config-ok")
        }
    } else {
        view.errors
            .iter()
            .map(|err| format!("{}: {}", err.path, err.message))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn show_mcp_form(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    ui.heading(i18n::fl("plugins-mcp"));
    show_mcp_catalog(ui, state, client, rt, async_results);
    let mut remove = None;
    for (index, server) in state.mcp_servers.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(i18n::fl("plugins-mcp-id"));
                ui.text_edit_singleline(&mut server.id);
                ui.checkbox(&mut server.enabled, i18n::fl("plugins-mcp-enabled"));
                if ui.button(i18n::fl("plugins-mcp-remove")).clicked() {
                    remove = Some(index);
                }
            });
            ui.horizontal(|ui| {
                ui.label(i18n::fl("plugins-mcp-transport"));
                let label = if server.transport == "stdio" || server.transport.is_empty() {
                    i18n::fl("plugins-mcp-stdio")
                } else {
                    i18n::fl("plugins-mcp-http")
                };
                egui::ComboBox::from_id_salt(("mcp-transport", index))
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                server.transport == "stdio" || server.transport.is_empty(),
                                i18n::fl("plugins-mcp-stdio"),
                            )
                            .clicked()
                        {
                            "stdio".clone_into(&mut server.transport);
                        }
                        if ui
                            .selectable_label(
                                server.transport == "http",
                                i18n::fl("plugins-mcp-http"),
                            )
                            .clicked()
                        {
                            "http".clone_into(&mut server.transport);
                        }
                    });
            });
            if server.transport == "http"
                || server.transport == "sse"
                || server.transport == "streamable_http"
                || server.transport == "streamable-http"
            {
                ui.horizontal(|ui| {
                    ui.label(i18n::fl("plugins-mcp-url"));
                    let url = server.url.get_or_insert_with(String::new);
                    ui.text_edit_singleline(url);
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(i18n::fl("plugins-mcp-command"));
                    let command = server.command.get_or_insert_with(String::new);
                    ui.text_edit_singleline(command);
                });
                ui.label(i18n::fl("plugins-mcp-args"));
                let mut args = mcp_args_text(&server.args);
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut args)
                            .desired_width(f32::INFINITY)
                            .desired_rows(3)
                            .hint_text(i18n::fl("plugins-mcp-args-hint")),
                    )
                    .changed()
                {
                    set_mcp_args_text(server, &args);
                }
            }
        });
    }
    if let Some(index) = remove {
        state.mcp_servers.remove(index);
    }
    if ui.button(i18n::fl("plugins-mcp-add")).clicked() {
        state.mcp_servers.push(ene_api::McpServerView {
            id: String::new(),
            transport: "stdio".to_owned(),
            command: Some(String::new()),
            args: Vec::new(),
            url: None,
            enabled: true,
        });
    }
    if ui.button(i18n::fl("plugins-mcp-save")).clicked() {
        if let Err(err) = validate_mcp_document(&state.mcp_servers) {
            state.core_status = err;
        } else {
            let doc = ene_api::McpDocument {
                servers: state.mcp_servers.clone(),
            };
            if let Ok(pretty) = serde_json::to_string_pretty(&doc) {
                pretty.clone_into(&mut state.mcp_json);
            }
            let client = Arc::clone(client);
            spawn_async(rt, async_results, async move {
                AsyncOutcome::SaveMcp(
                    client
                        .put_mcp(&doc)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }
    ui.collapsing(i18n::fl("plugins-mcp-json"), |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut state.mcp_json)
                .desired_width(f32::INFINITY)
                .desired_rows(6),
        );
        if ui.button(i18n::fl("plugins-mcp-from-json")).clicked() {
            match load_mcp_form(state, &state.mcp_json.clone()) {
                Ok(()) => state.core_status = i18n::fl("settings-loaded"),
                Err(err) => state.core_status = err,
            }
        }
    });
}

fn show_mcp_catalog(
    ui: &mut egui::Ui,
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if state.mcp_catalog.is_empty() {
        return;
    }
    ui.collapsing(i18n::fl("mcp-catalog-title"), |ui| {
        ui.label(i18n::fl("mcp-catalog-hint"));
        ui.weak(format!(
            "{}: {} / {}: {}",
            i18n::fl("mcp-catalog-source"),
            state.mcp_catalog_source,
            i18n::fl("mcp-catalog-fallback"),
            state.mcp_catalog_fallback
        ));
        for entry in &state.mcp_catalog.clone() {
            let selected = state.mcp_selected_catalog_id == entry.id;
            if ui
                .selectable_label(selected, format!("{} — {}", entry.label, entry.description))
                .clicked()
            {
                state.mcp_selected_catalog_id = if selected {
                    String::new()
                } else {
                    entry.id.clone()
                };
            }
            if selected {
                if entry.auth != ene_api::McpCatalogAuthView::None {
                    ui.horizontal(|ui| {
                        ui.label(i18n::fl("mcp-catalog-auth-token"));
                        let field = egui::TextEdit::singleline(&mut state.mcp_catalog_auth_input)
                            .password(true)
                            .hint_text(i18n::fl("mcp-catalog-auth-token-hint"));
                        ui.add(field);
                    });
                }
                egui::Grid::new(("mcp-catalog-detail", entry.id.as_str()))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(i18n::fl("plugins-mcp-id"));
                        ui.label(&entry.id);
                        ui.end_row();
                        ui.label(i18n::fl("plugins-mcp-transport"));
                        ui.label(&entry.transport);
                        ui.end_row();
                        if let Some(command) = &entry.command {
                            ui.label(i18n::fl("plugins-mcp-command"));
                            ui.label(format!("{command} {}", entry.args.join(" ")));
                            ui.end_row();
                        }
                        if let Some(url) = &entry.url {
                            ui.label(i18n::fl("plugins-mcp-url"));
                            ui.hyperlink_to(url, url);
                            ui.end_row();
                        }
                        ui.label(i18n::fl("mcp-catalog-auth"));
                        ui.label(match entry.auth {
                            ene_api::McpCatalogAuthView::None => i18n::fl("mcp-catalog-auth-none"),
                            ene_api::McpCatalogAuthView::ApiKeyHeader => {
                                i18n::fl("mcp-catalog-auth-api-key")
                            }
                            ene_api::McpCatalogAuthView::Oauth2Remote => {
                                i18n::fl("mcp-catalog-auth-oauth")
                            }
                        });
                        ui.end_row();
                        ui.label(i18n::fl("mcp-catalog-side-effects"));
                        ui.label(entry.side_effects.join("; "));
                        ui.end_row();
                        ui.label(i18n::fl("mcp-catalog-source-url"));
                        ui.hyperlink_to(&entry.source_url, &entry.source_url);
                        ui.end_row();
                    });
                let connect_clicked = ui.button(i18n::fl("mcp-catalog-connect")).clicked();
                let already_added = state.mcp_servers.iter().any(|server| server.id == entry.id);
                if connect_clicked && !already_added {
                    let generation = state.next_mcp_probe_generation();
                    state.mcp_probe_pending = Some(entry.id.clone());
                    state.mcp_probe_result = None;
                    // Snapshot the catalog row up front so Add keeps the
                    // probed configuration even after the pending flag clears.
                    state.mcp_probe_candidate = Some(entry.clone());
                    let probe = ene_api::McpProbeRequest {
                        id: entry.id.clone(),
                        transport: entry.transport.clone(),
                        command: entry.command.clone(),
                        args: entry.args.clone(),
                        url: entry.url.clone(),
                        auth_token: {
                            let token = state.mcp_catalog_auth_input.trim().to_owned();
                            (!token.is_empty()).then_some(token)
                        },
                    };
                    // Keep the credential only long enough for the probe request.
                    state.mcp_catalog_auth_input.clear();
                    let client_probe = Arc::clone(client);
                    spawn_async(rt, async_results, async move {
                        AsyncOutcome::ProbeMcp {
                            generation,
                            result: client_probe
                                .probe_mcp(&probe)
                                .await
                                .map_err(|e| e.to_string()),
                        }
                    });
                } else if connect_clicked && already_added {
                    state.core_status = i18n::fl("mcp-catalog-already-added");
                }
            }
        }
    });
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
                .selected_text(theme_label(&local_settings.theme))
                .show_ui(ui, |ui| {
                    for theme in ["system", "dark", "light"] {
                        ui.selectable_value(
                            &mut local_settings.theme,
                            theme.to_owned(),
                            theme_label(theme),
                        );
                    }
                });
            ui.end_row();
            ui.label(i18n::fl("settings-language"));
            let selected_language_label = language_value_label(&local_settings.language);
            egui::ComboBox::from_id_salt("language")
                .selected_text(selected_language_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut local_settings.language,
                        String::new(),
                        i18n::fl("settings-language-system"),
                    );
                    ui.selectable_value(
                        &mut local_settings.language,
                        "ja".to_owned(),
                        language_value_label("ja"),
                    );
                    ui.selectable_value(
                        &mut local_settings.language,
                        "en-US".to_owned(),
                        language_value_label("en-US"),
                    );
                });
            ui.end_row();
            ui.label(i18n::fl("settings-core-lifetime"));
            egui::ComboBox::from_id_salt("core-lifetime")
                .selected_text(core_lifetime_label(&local_settings.core_lifetime))
                .show_ui(ui, |ui| {
                    for value in ["app", "detached"] {
                        ui.selectable_value(
                            &mut local_settings.core_lifetime,
                            value.to_owned(),
                            core_lifetime_label(value),
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
            state.invalidate_settings();
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
    ui.separator();
    ui.heading(i18n::fl("settings-core-common"));
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-plugins-profile"));
        for profile in ["desktop", "minimal", "headless"] {
            if ui
                .selectable_label(
                    state.plugins_profile == profile,
                    plugin_profile_label(profile),
                )
                .clicked()
            {
                profile.clone_into(&mut state.plugins_profile);
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(i18n::fl("settings-approval-mode"));
        let current = normalize_approval_mode(&state.approval_mode);
        let label = approval_mode_label(&current);
        egui::ComboBox::from_id_salt("approval-mode")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (id, key) in [
                    ("policy", "settings-approval-policy"),
                    ("ask_all", "settings-approval-ask"),
                    ("auto", "settings-approval-auto"),
                    ("ai_auto", "settings-approval-ai"),
                ] {
                    if ui.selectable_label(current == id, i18n::fl(key)).clicked() {
                        id.clone_into(&mut state.approval_mode);
                    }
                }
            });
    });
    if matches!(
        normalize_approval_mode(&state.approval_mode).as_str(),
        "auto" | "ai_auto"
    ) {
        danger_hint(ui, &i18n::fl("approval-auto-warning"));
    }
    if ui.button(i18n::fl("settings-apply-core-fields")).clicked() {
        let profile = state.plugins_profile.clone();
        let mode = normalize_approval_mode(&state.approval_mode);
        let client = Arc::clone(client);
        spawn_async(rt, async_results, async move {
            AsyncOutcome::ApplyCoreSettings(
                client
                    .patch_settings(&serde_json::json!({
                        "plugins": { "profile": profile },
                        "approval": { "mode": mode },
                    }))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            )
        });
    }
    ui.collapsing(i18n::fl("settings-core-json-fold"), |ui| {
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
                            let patch: Value = serde_json::from_str(&text).map_err(|err| {
                                format!("{}: {err}", i18n::fl("settings-patch-invalid"))
                            })?;
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

fn approval_mode_label(mode: &str) -> String {
    match mode {
        "ask_all" => i18n::fl("settings-approval-ask"),
        "auto" => i18n::fl("settings-approval-auto"),
        "ai_auto" => i18n::fl("settings-approval-ai"),
        _ => i18n::fl("settings-approval-policy"),
    }
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
            let prefix = log_kind_label(entry.kind);
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

pub(crate) fn ensure_settings(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    if !state.begin_settings_load() {
        return;
    }
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
    require_chat: bool,
) {
    if require_chat && let Some(reason) = chat_apply_block_reason(state) {
        state.core_status = reason;
        return;
    }
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

fn voice_settings_patch(state: &mut DetailUiState) -> Value {
    let mut tts = serde_json::json!({
        "plugin": state.tts_plugin,
        "model": state.tts_model,
        "base_url": state.tts_base_url,
        "voice": state.tts_voice,
    });
    if !state.tts_api_key.is_empty()
        && let Some(object) = tts.as_object_mut()
    {
        object.insert(
            "api_key".to_owned(),
            Value::String(std::mem::take(&mut state.tts_api_key)),
        );
    } else if state.tts_api_key_clear_pending
        && let Some(object) = tts.as_object_mut()
    {
        object.insert("api_key".to_owned(), Value::Null);
    }
    let mut stt = serde_json::json!({
        "plugin": state.stt_plugin,
        "model": state.stt_model,
        "base_url": state.stt_base_url,
    });
    if !state.stt_api_key.is_empty()
        && let Some(object) = stt.as_object_mut()
    {
        object.insert(
            "api_key".to_owned(),
            Value::String(std::mem::take(&mut state.stt_api_key)),
        );
    } else if state.stt_api_key_clear_pending
        && let Some(object) = stt.as_object_mut()
    {
        object.insert("api_key".to_owned(), Value::Null);
    }
    serde_json::json!({
        "ai": {
            "tasks": {
                "tts": tts,
                "stt": stt,
            }
        }
    })
}

fn apply_voice_patch(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let patch = voice_settings_patch(state);
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

fn apply_observation_patch(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let title_mode = normalize_title_mode(&state.observation_title_mode);
    let patch = serde_json::json!({
        "mind": {
            "proactive": {
                "world_state": {
                    "title_mode": title_mode,
                    "ocr_hint": state.observation_ocr_hint
                }
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
    fn detail_value_labels_do_not_expose_storage_ids() {
        for (value, label) in [
            ("bottom", caption_position_label("bottom")),
            ("dark", theme_label("dark")),
            ("ja", language_value_label("ja")),
            ("detached", core_lifetime_label("detached")),
            ("minimal", plugin_profile_label("minimal")),
            ("classifier", optional_task_label("classifier")),
        ] {
            assert_ne!(label, value);
        }
        for (kind, value) in [
            (LogKind::Thinking, "thinking"),
            (LogKind::Inner, "inner"),
            (LogKind::Tool, "tool"),
            (LogKind::Session, "session"),
            (LogKind::Job, "job"),
            (LogKind::Affect, "affect"),
        ] {
            assert_ne!(log_kind_label(kind), value);
        }
        assert_eq!(optional_task_label("plugin.custom"), "plugin.custom");
    }

    #[test]
    fn candidate_drafts_follow_pending_identity_and_keep_edits() {
        let candidate = MemoryCandidateView {
            id: "candidate-1".to_owned(),
            soul_id: "soul".to_owned(),
            scope: "shared".to_owned(),
            kind: "semantic".to_owned(),
            title: "Original title".to_owned(),
            content: "Original content".to_owned(),
            confidence: 0.8,
            sensitive: false,
            expires_at: None,
        };
        let mut state = DetailUiState::default();

        state.sync_candidate_drafts(std::slice::from_ref(&candidate));
        state
            .candidate_drafts
            .get_mut("candidate-1")
            .expect("candidate draft is created with its pending row")
            .title = "Edited title".to_owned();
        state.sync_candidate_drafts(std::slice::from_ref(&candidate));

        assert_eq!(
            state
                .candidate_drafts
                .get("candidate-1")
                .map(|draft| draft.title.as_str()),
            Some("Edited title")
        );
        state.sync_candidate_drafts(&[]);
        assert!(state.candidate_drafts.is_empty());
    }

    #[test]
    fn pending_count_matches_list_length_and_empty_state() {
        let mut state = DetailUiState::default();
        assert_eq!(state.pending_count(), 0);

        let candidate = MemoryCandidateView {
            id: "candidate-1".to_owned(),
            soul_id: "soul".to_owned(),
            scope: "private".to_owned(),
            kind: "semantic".to_owned(),
            title: "T".to_owned(),
            content: "C".to_owned(),
            confidence: 0.5,
            sensitive: false,
            expires_at: None,
        };
        state.pending_memories.push(candidate.clone());
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn resolve_removes_candidate_optimistically_by_id() {
        let mut state = DetailUiState::default();
        let candidate = |id: &str| MemoryCandidateView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            scope: "shared".to_owned(),
            kind: "semantic".to_owned(),
            title: format!("Title {id}"),
            content: "C".to_owned(),
            confidence: 0.9,
            sensitive: true,
            expires_at: None,
        };
        let candidates = vec![candidate("a"), candidate("b")];
        state.sync_candidate_drafts(&candidates);
        state.pending_memories = candidates;
        state.shared_accept_armed.insert("b".to_owned());
        assert_eq!(state.pending_memories.len(), 2);

        state.remove_candidate("a");
        assert_eq!(
            state
                .pending_memories
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        assert!(state.shared_accept_armed.contains("b"));
        state.remove_candidate("b");
        assert!(state.pending_memories.is_empty());
        assert!(state.candidate_drafts.is_empty());
        assert!(!state.shared_accept_armed.contains("b"));
    }

    #[test]
    fn shared_scope_is_detected_from_original_or_edit() {
        let candidate = |scope: &str| MemoryCandidateView {
            id: "candidate-1".to_owned(),
            soul_id: "soul".to_owned(),
            scope: scope.to_owned(),
            kind: "semantic".to_owned(),
            title: "T".to_owned(),
            content: "C".to_owned(),
            confidence: 0.7,
            sensitive: false,
            expires_at: None,
        };

        assert_eq!(candidate("shared").scope, "shared");
        assert_ne!(candidate("private").scope, "shared");
    }

    #[test]
    fn new_chat_button_is_guarded_by_shared_inflight_state() {
        let state = DetailUiState {
            new_session_inflight: true,
            ..Default::default()
        };

        let guarded = !state.new_session_inflight;
        assert!(!guarded, "Detail button must not start another split");
    }

    #[test]
    fn active_jobs_exclude_terminal_states() {
        let job = |status: &str| JobView {
            id: status.to_owned(),
            soul_id: "soul".to_owned(),
            title: "task".to_owned(),
            goal: String::new(),
            status: status.to_owned(),
            progress_fraction: None,
            progress_note: None,
        };
        let jobs = vec![
            job("created"),
            job("queued"),
            job("running"),
            job("completed"),
            job("failed"),
            job("cancelled"),
            job("interrupted"),
        ];

        let active = active_jobs(&jobs)
            .into_iter()
            .map(|job| job.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(active, ["created", "queued", "running"]);
    }

    #[test]
    fn recent_jobs_keeps_terminal_jobs_in_api_order_and_caps_the_list() {
        let job = |id: usize, status: &str| JobView {
            id: id.to_string(),
            soul_id: "soul".to_owned(),
            title: "task".to_owned(),
            goal: String::new(),
            status: status.to_owned(),
            progress_fraction: None,
            progress_note: None,
        };
        let jobs = vec![
            job(0, "completed"),
            job(1, "running"),
            job(2, "failed"),
            job(3, "cancelled"),
            job(4, "interrupted"),
            job(5, "completed"),
            job(6, "failed"),
        ];

        let recent = recent_jobs(&jobs)
            .into_iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(recent, ["0", "2", "3", "4", "5"]);
    }

    #[test]
    fn onboarding_follows_chat_readiness_and_local_dismissal() {
        let mut state = DetailUiState::default();
        state.finish_settings_load();
        let mut settings = DesktopSettings::default();

        assert!(onboarding_visible(&state, &settings));
        settings.onboarding_dismissed = true;
        assert!(!onboarding_visible(&state, &settings));

        settings.onboarding_dismissed = false;
        state.chat_plugin = "provider.gguf".to_owned();
        state.chat_model = "local-model".to_owned();
        assert!(!onboarding_visible(&state, &settings));
    }

    #[test]
    fn occupant_rows_hide_unresolved_entries_and_keep_package_with_avatar() {
        let occupants = vec![
            OccupantView {
                soul_id: "soul.text-only".to_owned(),
                body_id: None,
                package_id: None,
                avatar_path: None,
            },
            OccupantView {
                soul_id: "soul.avatar".to_owned(),
                body_id: Some("body".to_owned()),
                package_id: Some("char.alicia-b@1.0.0".to_owned()),
                avatar_path: Some("/packages/char.alicia-b@1.0.0/model.vrm".to_owned()),
            },
        ];

        let visible = renderable_occupants(&occupants).collect::<Vec<_>>();
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible[0].package_id.as_deref(),
            Some("char.alicia-b@1.0.0")
        );
    }

    #[test]
    fn activation_generation_rejects_stale_results() {
        let mut state = DetailUiState::default();
        let first = state.next_activation_generation();
        let second = state.next_activation_generation();

        assert!(!state.activation_is_current(first));
        assert!(state.activation_is_current(second));
    }

    #[test]
    fn parse_core_fields_reads_effective_tasks_and_profile() {
        let json = r#"{
            "overlay": {},
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": { "plugin": "openai", "model": "gpt", "base_url": "https://example.invalid/v1" },
                        "tts": { "plugin": "provider.elevenlabs", "model": "eleven", "base_url": "https://tts.example.invalid", "voice": "alloy" },
                        "stt": { "plugin": "provider.whisper", "model": "small", "base_url": "https://stt.example.invalid" },
                        "classifier": { "plugin": "echo" }
                    }
                },
                "plugins": { "profile": "desktop" },
                "ai_chat_key_set": true,
                "ai_tts_key_set": true,
                "ai_stt_key_set": false
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(state.chat_plugin, "openai");
        assert_eq!(state.chat_model, "gpt");
        assert_eq!(state.chat_base_url, "https://example.invalid/v1");
        assert!(state.ai_chat_key_set);
        assert_eq!(state.tts_plugin, "provider.elevenlabs");
        assert_eq!(state.tts_model, "eleven");
        assert_eq!(state.tts_base_url, "https://tts.example.invalid");
        assert_eq!(state.tts_voice, "alloy");
        assert!(state.ai_tts_key_set);
        assert_eq!(state.stt_plugin, "provider.whisper");
        assert_eq!(state.stt_model, "small");
        assert_eq!(state.stt_base_url, "https://stt.example.invalid");
        assert!(!state.ai_stt_key_set);
        assert_eq!(state.plugins_profile, "desktop");
        assert_eq!(state.approval_mode, "policy");
        assert_eq!(state.observation_title_mode, "app_only");
        assert!(!state.observation_ocr_hint);
        assert!(!state.unconfigured.iter().any(|task| task == "chat"));
        assert!(state.unconfigured.iter().any(|task| task == "classifier"));
        assert!(!state.unconfigured.iter().any(|task| task == "stt"));
        assert!(blocking_unconfigured(&state.unconfigured).is_empty());
        assert!(optional_unconfigured(&state.unconfigured).contains(&"classifier"));
    }

    #[test]
    fn chat_provider_choices_use_display_names_not_raw_ids() {
        let providers = serde_json::json!([
            {
                "id": "provider.openai_compat",
                "seams": ["seam.llm"],
                "needs_key": true,
                "local": false,
                "installed": true
            },
            {
                "id": "provider.elevenlabs",
                "seams": ["seam.tts"],
                "needs_key": true,
                "installed": true
            },
            {
                "id": "provider.gguf",
                "seams": ["seam.llm", "seam.embed"],
                "needs_key": false,
                "local": true,
                "installed": false
            }
        ]);
        let choices = chat_provider_choices(&providers);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, "provider.openai_compat");
        assert_eq!(choices[0].label, i18n::fl("provider-openai-compat"));
        assert_ne!(choices[0].label, "provider.openai_compat");
        assert_eq!(provider_display_name("provider.custom_llm"), "custom llm");
    }

    #[test]
    fn voice_provider_choices_filter_by_seam_and_install_state() {
        let providers = serde_json::json!([
            {
                "id": "provider.elevenlabs",
                "seams": ["seam.tts"],
                "needs_key": true,
                "installed": true
            },
            {
                "id": "provider.whisper",
                "seams": ["seam.stt"],
                "needs_key": false,
                "installed": true
            },
            {
                "id": "provider.missing",
                "seams": ["seam.tts", "seam.stt"],
                "installed": false
            },
            {
                "id": "provider.llm",
                "seams": ["seam.llm"],
                "installed": true
            }
        ]);
        assert_eq!(
            provider_choices_for_seam(&providers, "seam.tts")[0].id,
            "provider.elevenlabs"
        );
        assert_eq!(
            provider_choices_for_seam(&providers, "seam.stt")[0].id,
            "provider.whisper"
        );
        assert!(provider_choices_for_seam(&providers, "seam.embed").is_empty());
    }

    #[test]
    fn voice_patch_is_scoped_and_does_not_send_empty_secrets() {
        let mut state = DetailUiState {
            chat_plugin: "provider.chat".to_owned(),
            tts_plugin: "provider.elevenlabs".to_owned(),
            tts_model: "eleven".to_owned(),
            tts_base_url: "https://tts.example.invalid".to_owned(),
            tts_voice: "alloy".to_owned(),
            stt_plugin: "provider.whisper".to_owned(),
            stt_model: "small".to_owned(),
            stt_api_key: "stt-secret".to_owned(),
            ..DetailUiState::default()
        };
        let patch = voice_settings_patch(&mut state);
        assert!(patch.pointer("/ai/tasks/chat").is_none());
        assert_eq!(
            patch.pointer("/ai/tasks/tts/plugin"),
            Some(&serde_json::json!("provider.elevenlabs"))
        );
        assert!(patch.pointer("/ai/tasks/tts/api_key").is_none());
        assert_eq!(
            patch.pointer("/ai/tasks/stt/api_key"),
            Some(&serde_json::json!("stt-secret"))
        );
        assert!(state.stt_api_key.is_empty());
    }

    #[test]
    fn switching_voice_provider_emits_an_explicit_secret_clear() {
        let mut state = DetailUiState {
            tts_plugin: "provider.new".to_owned(),
            tts_api_key_clear_pending: true,
            stt_plugin: "provider.stt".to_owned(),
            ..DetailUiState::default()
        };
        let patch = voice_settings_patch(&mut state);
        assert_eq!(
            patch.pointer("/ai/tasks/tts/api_key"),
            Some(&serde_json::Value::Null)
        );
        assert!(patch.pointer("/ai/tasks/stt/api_key").is_none());
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
                    {
                        "id": "provider.openai_compat",
                        "seams": ["seam.llm"],
                        "needs_key": true,
                        "installed": true
                    },
                    { "id": "provider.gguf", "needs_key": false }
                ]
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(chat_setup_gap(&state), Some(ChatSetupGap::ApiKey));
        assert!(blocking_unconfigured(&state.unconfigured).contains(&"chat"));
        assert_eq!(home_chat_next_step(&state), i18n::fl("home-next-chat-key"));
        assert_eq!(
            chat_apply_block_reason(&state),
            Some(i18n::fl("settings-chat-key-required"))
        );
        state.chat_api_key = "placeholder-key".to_owned();
        assert!(chat_apply_block_reason(&state).is_none());
    }

    #[test]
    fn home_cards_share_conversation_and_voice_readiness() {
        let json = r#"{
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": { "plugin": "provider.openai_compat", "model": "gpt" },
                        "tts": { "plugin": "echo" },
                        "stt": { "plugin": "echo" }
                    }
                },
                "ai_chat_key_set": false,
                "providers": [
                    {
                        "id": "provider.openai_compat",
                        "seams": ["seam.llm"],
                        "needs_key": true,
                        "installed": true
                    }
                ]
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);

        let cards = setup_cards(&state);
        assert!(cards.contains(&SetupCard {
            tab: DetailTab::Conversation,
            state: SetupState::NeedsSetup,
        }));
        assert!(cards.contains(&SetupCard {
            tab: DetailTab::Voice,
            state: SetupState::NeedsSetup,
        }));
        assert_eq!(
            home_chat_next_step(&state),
            i18n::fl("home-next-chat-key"),
            "Home and Conversation must use the same chat readiness source"
        );
    }

    #[test]
    fn configured_chat_and_voice_show_ready_cards() {
        let json = r#"{
            "effective": {
                "ai": {
                    "tasks": {
                        "chat": { "plugin": "provider.gguf", "model": "local.gguf" },
                        "tts": { "plugin": "provider.voicevox" },
                        "stt": { "plugin": "provider.openai_compat" }
                    }
                },
                "ai_chat_key_set": false,
                "providers": [
                    { "id": "provider.gguf", "needs_key": false, "installed": true },
                    { "id": "provider.voicevox", "needs_key": false, "installed": true },
                    { "id": "provider.openai_compat", "needs_key": false, "installed": true }
                ]
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);

        let cards = setup_cards(&state);
        assert!(cards.contains(&SetupCard {
            tab: DetailTab::Conversation,
            state: SetupState::Ready,
        }));
        assert!(cards.contains(&SetupCard {
            tab: DetailTab::Voice,
            state: SetupState::Ready,
        }));
    }

    #[test]
    fn fallback_voice_plugins_treat_blank_fields_as_valid() {
        assert!(plugin_has_fallback("provider.edge_tts"));
        assert!(plugin_has_fallback("provider.voicevox"));
        assert!(plugin_has_fallback("provider.openai_compat"));
        assert!(!plugin_has_fallback("provider.elevenlabs"));
        assert!(!plugin_has_fallback(""));
    }

    #[test]
    fn reopening_detail_refreshes_stale_vault_state() {
        let mut state = DetailUiState {
            visible: true,
            ..DetailUiState::default()
        };
        parse_core_fields(
            r#"{
                "effective": {
                    "ai": {"tasks": {"chat": {"plugin": "provider.openai_compat", "model": "m"}}},
                    "ai_chat_key_set": false,
                    "providers": [{"id": "provider.openai_compat", "needs_key": true}]
                }
            }"#,
            &mut state,
        );
        assert_eq!(chat_setup_gap(&state), Some(ChatSetupGap::ApiKey));

        // Simulate a later settings load after an external vault write.
        state.finish_settings_load();
        state.refresh_settings_on_open();
        assert!(!state.settings_loaded());
    }

    #[test]
    fn settings_loading_state_prevents_duplicate_requests() {
        let mut state = DetailUiState::default();
        assert!(!state.settings_loaded());
        assert!(state.begin_settings_load());
        assert!(!state.settings_loaded());
        assert!(!state.begin_settings_load());

        state.finish_settings_load();
        assert!(state.settings_loaded());
        assert!(!state.begin_settings_load());

        state.invalidate_settings();
        assert!(!state.settings_loaded());
        assert!(state.begin_settings_load());
    }

    #[test]
    fn unconfigured_chat_stays_blocked_after_settings_hydration() {
        let mut state = DetailUiState::default();
        state.finish_settings_load();
        assert_eq!(chat_setup_gap(&state), Some(ChatSetupGap::Plugin));
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
    fn observation_privacy_is_on_conversation_tab() {
        assert!(DetailTab::Conversation.matches_search("observation"));
        assert!(DetailTab::Conversation.matches_search("privacy"));
        let json = r#"{
            "overlay": {},
            "effective": {
                "mind": {
                    "proactive": {
                        "world_state": {
                            "title_mode": "redacted_title",
                            "ocr_hint": true
                        }
                    }
                }
            }
        }"#;
        let mut state = DetailUiState::default();
        parse_core_fields(json, &mut state);
        assert_eq!(state.observation_title_mode, "redacted_title");
        assert!(state.observation_ocr_hint);
        assert!(
            observation_scope_text(&state).contains(&i18n::fl("settings-observation-redacted"))
        );
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
                last_error: None,
            },
            PluginView {
                row_id: "2".into(),
                plugin: "provider.openai_compat".into(),
                state: "ready".into(),
                wait_reason: None,
                last_error: None,
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
            skill_refs: Vec::new(),
        };
        assert!(character_export_package_id(Some(&unbound)).is_none());
        let mut packaged = unbound.clone();
        packaged.package_id = Some("char.alicia@1.0.0".into());
        assert_eq!(
            character_export_package_id(Some(&packaged)).as_deref(),
            Some("char.alicia")
        );
    }

    #[test]
    fn mcp_form_rejects_incomplete_servers_and_loads_json() {
        let incomplete = ene_api::McpServerView {
            id: String::new(),
            transport: "stdio".to_owned(),
            command: Some("npx".to_owned()),
            args: Vec::new(),
            url: None,
            enabled: true,
        };
        assert!(validate_mcp_server(&incomplete).is_err());
        let stdio = ene_api::McpServerView {
            id: "files".to_owned(),
            transport: "stdio".to_owned(),
            command: Some("npx".to_owned()),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-filesystem".to_owned(),
            ],
            url: None,
            enabled: true,
        };
        assert!(validate_mcp_server(&stdio).is_ok());
        let mut state = DetailUiState::default();
        load_mcp_form(
            &mut state,
            r#"{"servers":[{"id":"web","transport":"http","url":"http://127.0.0.1:9"}]}"#,
        )
        .expect("json");
        assert_eq!(state.mcp_servers.len(), 1);
        assert_eq!(state.mcp_servers[0].id, "web");
        assert!(validate_mcp_document(&state.mcp_servers).is_ok());
        assert_eq!(normalize_approval_mode("AskAll"), "ask_all");
        assert_eq!(normalize_approval_mode("policy"), "policy");
        let mut spaced = stdio;
        set_mcp_args_text(&mut spaced, "-y\nC:\\Users\\Jane Doe\\workspace\n--verbose");
        assert_eq!(
            spaced.args,
            vec![
                "-y".to_owned(),
                r"C:\Users\Jane Doe\workspace".to_owned(),
                "--verbose".to_owned(),
            ]
        );
        assert_eq!(
            mcp_args_text(&spaced.args),
            "-y\nC:\\Users\\Jane Doe\\workspace\n--verbose"
        );
    }

    #[test]
    fn plugin_config_view_fills_editor_without_secret_values() {
        let mut state = DetailUiState::default();
        apply_plugin_config_view(
            &mut state,
            PluginConfigView {
                row_id: "r-demo".to_owned(),
                plugin: "tool.demo".to_owned(),
                has_config: true,
                schema: serde_json::json!({"type":"object"}),
                values: serde_json::json!({"model":"ok"}),
                secret_keys: vec!["api_key".to_owned()],
            },
        );
        assert_eq!(state.plugin_config_id, "r-demo");
        assert!(state.plugin_config_has);
        assert!(state.plugin_config_values.contains("ok"));
        assert!(!state.plugin_config_values.contains("sk-"));
        assert_eq!(state.plugin_config_secrets, vec!["api_key".to_owned()]);
        let failed = ene_api::PluginConfigValidateView {
            ok: false,
            errors: vec![ene_api::PluginConfigErrorView {
                path: "model".to_owned(),
                message: "unknown".to_owned(),
            }],
            restart_required: false,
        };
        assert!(plugin_config_status(&failed).contains("model"));
    }

    #[test]
    fn selecting_plugin_config_clears_editor_until_load_finishes() {
        let mut detail = DetailUiState {
            plugin_config_id: "plugin.a".to_owned(),
            plugin_config_schema: r#"{"properties":{"old":{}}}"#.to_owned(),
            plugin_config_values: r#"{"old":"value"}"#.to_owned(),
            plugin_config_secrets: vec!["old_secret".to_owned()],
            plugin_config_options_field: "old".to_owned(),
            plugin_config_options: "old-option".to_owned(),
            ..Default::default()
        };

        let request_id = begin_plugin_config_load(&mut detail, "plugin.b");

        assert_eq!(request_id, 1);
        assert_eq!(detail.plugin_config_id, "plugin.b");
        assert!(plugin_config_is_loading(&detail));
        assert!(detail.plugin_config_schema.is_empty());
        assert!(detail.plugin_config_values.is_empty());
        assert!(detail.plugin_config_secrets.is_empty());
        assert!(detail.plugin_config_options_field.is_empty());
        assert!(detail.plugin_config_options.is_empty());
    }

    #[test]
    fn plugin_config_values_disable_empty_and_invalid_input() {
        for (values, can_submit) in [
            ("", false),
            ("  \n", false),
            ("{}", false),
            (" { } ", false),
            ("{", false),
            (r#"{"model":"ok"}"#, true),
        ] {
            assert_eq!(
                plugin_config_values_valid(values) && !plugin_config_values_empty(values),
                can_submit,
                "unexpected submit state for {values:?}"
            );
        }
    }

    #[test]
    fn stale_plugin_config_response_cannot_replace_newer_request() {
        let mut detail = DetailUiState::default();
        let stale_request_id = begin_plugin_config_load(&mut detail, "plugin.a");
        let current_request_id = begin_plugin_config_load(&mut detail, "plugin.b");
        let view = |id: &str, value: &str| PluginConfigView {
            row_id: id.to_owned(),
            plugin: id.to_owned(),
            has_config: true,
            schema: serde_json::json!({"type":"object"}),
            values: serde_json::json!({"value": value}),
            secret_keys: Vec::new(),
        };

        let stale = view("plugin.a", "old");
        if plugin_config_load_is_current(&detail, &stale.row_id, stale_request_id) {
            apply_plugin_config_view(&mut detail, stale);
        }
        assert!(detail.plugin_config_values.is_empty());

        let current = view("plugin.b", "new");
        if plugin_config_load_is_current(&detail, &current.row_id, current_request_id) {
            apply_plugin_config_view(&mut detail, current);
            detail.plugin_config_loading_request_id = None;
        }
        assert!(detail.plugin_config_values.contains("new"));
        assert!(!detail.plugin_config_values.contains("old"));
    }

    #[test]
    fn add_probed_server_persists_the_exact_catalog_entry_disabled() {
        let mut state = DetailUiState::default();
        let candidate = ene_api::McpCatalogEntryView {
            id: "github-remote".to_owned(),
            label: "GitHub".to_owned(),
            description: String::new(),
            transport: "http".to_owned(),
            command: None,
            args: Vec::new(),
            url: Some("https://mcp.example.com/sse".to_owned()),
            auth: ene_api::McpCatalogAuthView::Oauth2Remote,
            side_effects: Vec::new(),
            source_url: String::new(),
        };
        state.mcp_probe_candidate = Some(candidate.clone());

        add_probed_server(&mut state, &candidate);

        assert_eq!(state.mcp_servers.len(), 1);
        let added = &state.mcp_servers[0];
        assert_eq!(added.id, candidate.id);
        assert_eq!(added.transport, candidate.transport);
        assert_eq!(added.command, candidate.command);
        assert_eq!(added.args, candidate.args);
        assert_eq!(added.url, candidate.url);
        assert!(
            !added.enabled,
            "added catalog rows must stay disabled until reviewed"
        );
        assert!(state.mcp_probe_candidate.is_none());
        assert!(state.mcp_probe_result.is_none());
    }

    #[test]
    fn stale_mcp_probe_results_are_ignored() {
        let mut state = DetailUiState::default();
        let first = state.next_mcp_probe_generation();
        let second = state.next_mcp_probe_generation();

        assert!(!state.mcp_probe_is_current(first));
        assert!(state.mcp_probe_is_current(second));
    }

    #[test]
    fn character_active_badge_matches_active_soul() {
        let active = CharacterView {
            id: "char.alicia".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.alicia".to_owned(),
            soul_id: Some("alicia".to_owned()),
        };
        let other = CharacterView {
            id: "char.alicia-b".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.alicia-b".to_owned(),
            soul_id: Some("alicia-b".to_owned()),
        };
        let unbound = CharacterView {
            id: "char.orphan".to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.orphan".to_owned(),
            soul_id: None,
        };

        assert!(is_character_active(&active, Some("alicia")));
        assert!(!is_character_active(&other, Some("alicia")));
        assert!(!is_character_active(&unbound, Some("alicia")));
        assert!(!is_character_active(&active, None));
    }
}
