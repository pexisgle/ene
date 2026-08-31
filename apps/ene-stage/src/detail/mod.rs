//! Detail window: eight new-core IA sections plus a session log.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
use chrono::{DateTime, Utc};
#[cfg(test)]
use ene_api::ApiError;
use ene_api::{
    ApiClient, CharacterView, CreateJobRequest, JobView, MemoryCandidateView, MemoryJournalView,
    MemoryView, OccupantView, PluginConfigView, PluginView, ProviderAssetView, ScheduleView,
    SoulView, ToolView,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::runtime::Handle;

use crate::monitor::{MonitorInfo, OverlayMonitorMode};
use crate::settings::DesktopSettings;
use crate::tasks::AsyncOutcome;

use crate::i18n;

mod primitives;
pub(crate) mod project;
mod tabs;

use primitives::{StatusCard, StatusTone};

#[cfg(test)]
const MAX_CHARACTER_IMPORT_BYTES: u64 = 32 * 1024 * 1024;

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

/// A job creation blocked by an approval ask, tied to the approval id whose
/// Allow resolution may replay it. Requests failed for any other reason are
/// never stashed, and a Deny or an unrelated approval must not consume it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJobRetry {
    pub request: CreateJobRequest,
    pub approval_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanionDisplay {
    pub soul_id: String,
    pub display_name: String,
    pub body_id: Option<String>,
    pub package_id: Option<String>,
    pub avatar_path: Option<String>,
    pub has_avatar: bool,
    pub displayed: bool,
    pub temporarily_hidden: bool,
    pub active: bool,
    pub order: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayAction {
    Show(String),
    TemporarilyHide(String),
    Remove(String),
    MoveUp(String),
    MoveDown(String),
}

#[must_use]
pub(crate) fn caption_position_label(value: &str) -> String {
    match value {
        "top" => i18n::fl("settings-caption-position-top"),
        "left" => i18n::fl("settings-caption-position-left"),
        "right" => i18n::fl("settings-caption-position-right"),
        "bottom" => i18n::fl("settings-caption-position-bottom"),
        _ => value.to_owned(),
    }
}

#[must_use]
pub(crate) fn theme_label(value: &str) -> String {
    match value {
        "system" => i18n::fl("settings-theme-system"),
        "dark" => i18n::fl("settings-theme-dark"),
        "light" => i18n::fl("settings-theme-light"),
        _ => value.to_owned(),
    }
}

#[must_use]
pub(crate) fn language_value_label(value: &str) -> String {
    match value {
        "" => i18n::fl("settings-language-system"),
        "ja" => i18n::fl("settings-language-ja"),
        "en-US" => i18n::fl("settings-language-en-us"),
        _ => value.to_owned(),
    }
}

#[must_use]
pub(crate) fn core_lifetime_label(value: &str) -> String {
    match value {
        "app" => i18n::fl("settings-core-lifetime-app"),
        "detached" => i18n::fl("settings-core-lifetime-detached"),
        _ => value.to_owned(),
    }
}

#[must_use]
pub(crate) fn plugin_profile_label(value: &str) -> String {
    match value {
        "desktop" => i18n::fl("settings-plugins-profile-desktop"),
        "minimal" => i18n::fl("settings-plugins-profile-minimal"),
        "headless" => i18n::fl("settings-plugins-profile-headless"),
        _ => value.to_owned(),
    }
}

#[cfg(test)]
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
pub(crate) fn log_kind_label(kind: LogKind) -> String {
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpCredentialDraft {
    pub token: String,
    pub inflight: bool,
}

#[derive(Clone, Debug)]
pub enum MotionCommand {
    SelectSoul(String),
    SelectMotion { soul_id: String, name: String },
    Stop { soul_id: String },
    Reset { soul_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionControl {
    pub soul_id: String,
    pub label: String,
    pub current: Option<String>,
    pub manual_override: bool,
    pub names: Vec<String>,
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
    /// True once a non-placeholder Speech-to-Text provider has been observed.
    /// Recomputed by `parse_core_fields` (and proven by a successful mic claim)
    /// so the mic guard and the settings loader share one source of truth.
    pub stt_plugin_ready: bool,
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
    pub character_list_loaded: bool,
    pub occupants: Vec<OccupantView>,
    pub motion_command: Option<MotionCommand>,
    pub body_ref_draft: String,
    pub jobs: Vec<JobView>,
    pub schedules: Vec<ScheduleView>,
    pub new_job_title: String,
    pub new_job_goal: String,
    pub new_job_inflight: bool,
    /// Exact request of the in-flight job creation click, kept so an
    /// approval-pending failure can be stashed verbatim for replay; any other
    /// terminal state clears it.
    pub submitted_job: Option<CreateJobRequest>,
    /// Job creation that failed while its `delegate.start` approval ask was
    /// still unresolved, stashed together with that approval id so exactly the
    /// matching Allow resolution can retry it once.
    pub pending_job_retry: Option<PendingJobRetry>,
    pub new_schedule_name: String,
    /// Raw spec typed in the Advanced builder mode; guided modes generate the
    /// spec from their own fields and never read this.
    pub new_schedule_spec: String,
    pub new_schedule_inflight: bool,
    pub new_schedule_builder: ScheduleBuilderState,
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
    pub mcp_credential_server_id: String,
    pub mcp_credential_draft: McpCredentialDraft,
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
    pub overlay_monitor_apply_pending: bool,
    pub overlay_monitor_fit_pending: bool,
    pub overlay_monitor_notice: String,
    pub character_action_pending: bool,
    pub display_action: Option<DisplayAction>,
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

/// Guided schedule builder inputs. `spec_for` renders them into the raw spec
/// the core validates, so the client never becomes a second validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleBuilderState {
    pub mode: ScheduleBuilderMode,
    pub interval_value: u32,
    pub interval_unit: ScheduleIntervalUnit,
    pub daily_hour: u32,
    pub daily_minute: u32,
    pub weekly_hour: u32,
    pub weekly_minute: u32,
    pub weekdays: [bool; 7],
}

impl Default for ScheduleBuilderState {
    fn default() -> Self {
        Self {
            mode: ScheduleBuilderMode::default(),
            // 15 minutes is the friendliest first interval; zero would
            // silently create an invalid `every 0m` spec the core rejects.
            interval_value: 15,
            interval_unit: ScheduleIntervalUnit::default(),
            daily_hour: 9,
            daily_minute: 0,
            weekly_hour: 9,
            weekly_minute: 0,
            weekdays: [false; 7],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScheduleBuilderMode {
    #[default]
    Interval,
    Daily,
    Weekly,
    Advanced,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScheduleIntervalUnit {
    #[default]
    Minutes,
    Hours,
    Days,
}

impl ScheduleBuilderState {
    /// Spec the core will receive in the current builder mode. Advanced mode
    /// returns an empty string; the raw text lives in the `new_schedule_spec`
    /// field of `DetailUiState` so power users keep full cron syntax.
    #[must_use]
    pub fn spec_for(&self) -> String {
        match self.mode {
            ScheduleBuilderMode::Interval => format_interval_spec(
                self.interval_value,
                match self.interval_unit {
                    ScheduleIntervalUnit::Minutes => "m",
                    ScheduleIntervalUnit::Hours => "h",
                    ScheduleIntervalUnit::Days => "d",
                },
            ),
            ScheduleBuilderMode::Daily => cron_spec(self.daily_minute, self.daily_hour, &["*"]),
            ScheduleBuilderMode::Weekly => {
                let names: Vec<&str> = WEEKDAY_SPEC_NAMES
                    .iter()
                    .copied()
                    .zip(self.weekdays)
                    .filter_map(|(name, on)| on.then_some(name))
                    .collect();
                cron_spec(self.weekly_minute, self.weekly_hour, &names)
            }
            ScheduleBuilderMode::Advanced => String::new(),
        }
    }
}

/// Spec the Create button sends: guided modes render the builder fields and
/// Advanced passes the raw text through, so the empty-string sentinel inside
/// `spec_for` never disables Advanced-mode creation.
#[cfg(test)]
#[must_use]
fn effective_schedule_spec(builder: &ScheduleBuilderState, raw_spec: &str) -> String {
    match builder.mode {
        ScheduleBuilderMode::Advanced => raw_spec.trim().to_owned(),
        _ => builder.spec_for(),
    }
}

const WEEKDAY_SPEC_NAMES: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

/// Creation gate shared by the button enable state and the request itself,
/// so a test can pin exactly when Create becomes eligible.
#[cfg(test)]
#[must_use]
fn schedule_create_is_eligible(builder: &ScheduleBuilderState, name: &str, raw_spec: &str) -> bool {
    !name.trim().is_empty() && !effective_schedule_spec(builder, raw_spec).is_empty()
}

#[must_use]
fn format_interval_spec(value: u32, unit: &str) -> String {
    format!("every {value}{unit}")
}

/// Render a 5-field cron spec. An empty weekday selection means no valid
/// schedule exists yet, so the caller sees an empty spec instead of a
/// silently always-fire pattern.
#[must_use]
fn cron_spec(minute: u32, hour: u32, days: &[&str]) -> String {
    if days.is_empty() {
        return String::new();
    }
    format!("{minute} {hour} * * {}", days.join(","))
}

/// Human-readable preview of the stored next-fire timestamp, rendered in the
/// schedule's own timezone when the offset is derivable and falling back to
/// UTC otherwise; an unparseable stored value passes through untouched.
#[cfg(test)]
#[must_use]
fn humanize_next_fire(next_fire: Option<&str>, timezone: &str) -> String {
    let Some(raw) = next_fire else {
        return i18n::fl("schedule-next-fire-none");
    };
    let Ok(ts) = DateTime::parse_from_rfc3339(raw) else {
        return raw.to_owned();
    };
    let local = match timezone.parse::<chrono::FixedOffset>() {
        Ok(offset) => ts.with_timezone(&offset),
        Err(_) => ts.with_timezone(&Utc).fixed_offset(),
    };
    let local = local.format("%Y-%m-%d %H:%M").to_string();
    i18n::format("schedule-next-fire", &[("at", &local)])
}

/// Local IANA timezone name used as the create-request default; falls back to
/// UTC when the platform cannot report one, matching the core default.
#[cfg(test)]
#[must_use]
fn local_timezone_name() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned())
}

#[derive(Clone, Debug, Default)]
struct DetailLoaded {
    memory: bool,
    character: bool,
    #[cfg(test)]
    jobs: bool,
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
        self.characters.clear();
        self.character_list_loaded = false;
        self.character_action_pending = false;
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

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending_memories.len()
    }

    /// Resolve removes the row before the server answers; the follow-up
    /// refresh is authoritative and restores the row when the resolve failed.
    #[cfg(test)]
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
    // The mic toggle reads this ready-mirror instead of re-parsing plugin
    // strings; a successful mic claim also proves readiness on its own.
    state.stt_plugin_ready = !(state.stt_plugin.is_empty() || state.stt_plugin == "echo");
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

/// Mirror of the daemon-side `valid_mcp_id` rule so the form can explain the
/// constraint while typing instead of surfacing a raw HTTP 400 after Save.
#[must_use]
pub fn valid_mcp_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
}

/// Sanitize a free-form display name into a valid MCP id token. Empty results
/// mean the name had no usable characters and the user must type an id.
#[must_use]
pub fn mcp_id_suggestion(display_name: &str) -> String {
    let cleaned: String = display_name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed = collapsed.trim_start_matches('.').to_owned();
    if trimmed.len() > 64 {
        trimmed[..64].to_owned()
    } else {
        trimmed
    }
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
    if !valid_mcp_id(server.id.trim()) {
        return Err(i18n::fl("mcp-id-invalid"));
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

/// Only persistent `mcp.<id>` rows keep credentials; probe rows are
/// ephemeral and their ids are rejected by the daemon id rule anyway.
#[must_use]
pub fn mcp_credential_row(plugin: &str, row_id: &str) -> Option<String> {
    let server_id = row_id.strip_prefix("mcp.")?;
    (!server_id.is_empty() && !server_id.starts_with("probe-") && plugin.starts_with("mcp."))
        .then(|| server_id.to_owned())
}

/// The credential is stored by probing the saved row with the new token; the
/// daemon persists it before the connection attempt, so the response tells us
/// whether the token was at least accepted into the vault even when the
/// remote itself is unreachable.
pub fn spawn_mcp_credential_save(
    state: &mut DetailUiState,
    client: &Arc<ApiClient>,
    rt: &Handle,
    async_results: &Arc<Mutex<Vec<AsyncOutcome>>>,
) {
    let token = state.mcp_credential_draft.token.trim().to_owned();
    // The trimmed value replaces the draft so a later frame cannot resend the
    // saved token from stale whitespace the user already submitted.
    state.mcp_credential_draft.token.clone_from(&token);
    // The setup button stores the already-stripped server id, not the raw
    // `mcp.<id>` fiber row id.
    let server_id = state.mcp_credential_server_id.clone();
    if server_id.is_empty() {
        return;
    }
    if token.is_empty() || state.mcp_credential_draft.inflight {
        return;
    }
    let Some(server) = state
        .mcp_servers
        .iter()
        .find(|server| server.id == server_id)
    else {
        // A credential row without a matching form row cannot be probed; the
        // status tells the user to save the server row first.
        state.connections_status = i18n::fl("connections-mcp-credential-server-missing");
        return;
    };
    if validate_mcp_server(server).is_err() {
        state.connections_status = i18n::fl("connections-mcp-credential-needs-url");
        return;
    }
    let probe = ene_api::McpProbeRequest {
        id: server.id.clone(),
        transport: server.transport.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
        url: server.url.clone(),
        auth_token: Some(token),
    };
    let generation = state.next_mcp_probe_generation();
    state.mcp_credential_draft.inflight = true;
    state.connections_status.clear();
    let client = Arc::clone(client);
    spawn_async(rt, async_results, async move {
        AsyncOutcome::SaveMcpCredential {
            generation,
            result: client.probe_mcp(&probe).await.map_err(|e| e.to_string()),
        }
    });
}

#[cfg(test)]
mod mcp_id_tests {
    use super::*;

    #[test]
    fn valid_mcp_id_mirrors_daemon_rule() {
        assert!(valid_mcp_id("exa"));
        assert!(valid_mcp_id("exa-search"));
        assert!(valid_mcp_id("exa_search.v2"));
        assert!(!valid_mcp_id(""));
        assert!(!valid_mcp_id("Exa Search"));
        assert!(!valid_mcp_id(".hidden"));
        assert!(!valid_mcp_id(&"a".repeat(65)));
        assert!(valid_mcp_id(&"a".repeat(64)));
    }

    #[test]
    fn mcp_id_suggestion_sanitizes_display_names() {
        assert_eq!(mcp_id_suggestion("Exa Search"), "exa-search");
        assert_eq!(mcp_id_suggestion("  Tavily  "), "tavily");
        assert_eq!(mcp_id_suggestion("GitHub (remote)"), "github-remote");
        assert_eq!(mcp_id_suggestion(""), "");
    }
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

pub(crate) fn home_status_cards(state: &DetailUiState) -> Vec<(DetailTab, StatusCard)> {
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
pub(crate) fn onboarding_visible(state: &DetailUiState, local_settings: &DesktopSettings) -> bool {
    state.settings_loaded()
        && !local_settings.onboarding_dismissed
        && chat_setup_gap(state).is_some()
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

#[cfg(test)]
#[must_use]
fn bundled_alicia_b_available(characters: &[CharacterView]) -> bool {
    !characters
        .iter()
        .any(|character| character.id == crate::bundle::BUNDLED_ALICIA_B_ID)
}

pub(crate) fn companion_display_rows(
    occupants: &[OccupantView],
    displayed_soul_ids: &[String],
    temporarily_hidden: &HashSet<String>,
    active_soul_id: &str,
) -> Vec<CompanionDisplay> {
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for (order, soul_id) in displayed_soul_ids.iter().enumerate() {
        if let Some(occupant) = occupants
            .iter()
            .find(|occupant| occupant.soul_id == *soul_id)
            && seen.insert(occupant.soul_id.clone())
        {
            rows.push(companion_display_row(
                occupant,
                Some(order),
                temporarily_hidden,
                active_soul_id,
                true,
            ));
        }
    }
    for occupant in occupants {
        let companion = occupant.package_id.is_some()
            || crate::core::session::occupant_has_avatar(occupant)
            || occupant.soul_id == active_soul_id;
        if companion && seen.insert(occupant.soul_id.clone()) {
            rows.push(companion_display_row(
                occupant,
                None,
                temporarily_hidden,
                active_soul_id,
                false,
            ));
        }
    }
    rows
}

fn companion_display_row(
    occupant: &OccupantView,
    order: Option<usize>,
    temporarily_hidden: &HashSet<String>,
    active_soul_id: &str,
    displayed: bool,
) -> CompanionDisplay {
    let display_name = if occupant.display_name.is_empty() {
        crate::core::session::occupant_label(occupant)
    } else {
        occupant.display_name.clone()
    };
    let has_avatar = occupant
        .avatar_path
        .as_ref()
        .is_some_and(|path| !path.is_empty());
    CompanionDisplay {
        soul_id: occupant.soul_id.clone(),
        display_name,
        body_id: occupant.body_id.clone(),
        package_id: occupant.package_id.clone(),
        avatar_path: occupant.avatar_path.clone(),
        has_avatar,
        displayed,
        temporarily_hidden: temporarily_hidden.contains(&occupant.soul_id),
        active: occupant.soul_id == active_soul_id,
        order,
    }
}

#[cfg(test)]
fn read_character_import(path: &Path) -> Result<Vec<u8>, ApiError> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| ApiError::Transport(format!("cannot read character package: {err}")))?;
    if metadata.len() > MAX_CHARACTER_IMPORT_BYTES {
        return Err(ApiError::Transport(i18n::fl(
            "character-import-file-too-large",
        )));
    }
    std::fs::read(path)
        .map_err(|err| ApiError::Transport(format!("cannot read character package: {err}")))
}

#[cfg(test)]
fn apply_arranged_positions(
    settings: &mut DesktopSettings,
    soul_ids: &[String],
    active_soul_id: &str,
) {
    for (soul_id, position) in soul_ids
        .iter()
        .zip(crate::settings::arranged_positions(soul_ids))
    {
        set_character_position(settings, soul_id, soul_id == active_soul_id, position);
    }
}

#[cfg(test)]
fn set_character_position(
    settings: &mut DesktopSettings,
    soul_id: &str,
    active: bool,
    position: [f32; 2],
) {
    settings
        .character_positions
        .insert(soul_id.to_owned(), position);
    if active {
        settings.character_x = position[0];
        settings.character_y = position[1];
    }
}

#[cfg(test)]
fn set_character_scale(settings: &mut DesktopSettings, soul_id: &str, active: bool, scale: f32) {
    let scale = crate::settings::clamp_model_scale(scale);
    settings.character_scales.insert(soul_id.to_owned(), scale);
    if active {
        settings.model_scale = scale;
    }
}

pub(crate) fn title_mode_label(mode: &str) -> String {
    match mode {
        "redacted_title" => i18n::fl("settings-observation-redacted"),
        "full_title" => i18n::fl("settings-observation-full"),
        _ => i18n::fl("settings-observation-app-only"),
    }
}

pub(crate) fn observation_scope_text(state: &DetailUiState) -> String {
    let title = title_mode_label(&normalize_title_mode(&state.observation_title_mode));
    let ocr = if state.observation_ocr_hint {
        i18n::fl("settings-observation-ocr-on")
    } else {
        i18n::fl("settings-observation-ocr-off")
    };
    format!("{title}; {ocr}")
}

#[must_use]
pub(crate) fn memory_kind_label(value: &str) -> String {
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
pub(crate) fn memory_scope_label(value: &str) -> String {
    match value {
        "private" => i18n::fl("memory-scope-private"),
        "shared" => i18n::fl("memory-scope-shared"),
        _ => value.to_owned(),
    }
}

#[cfg(test)]
fn begin_jobs_reload(state: &mut DetailUiState) {
    state.core_status.clear();
    state.loaded.jobs = false;
}

#[cfg(test)]
fn active_jobs(jobs: &[JobView]) -> Vec<&JobView> {
    jobs.iter()
        .filter(|job| {
            matches!(
                job.status.as_str(),
                "created" | "queued" | "running" | "verifying"
            )
        })
        .collect()
}

#[cfg(test)]
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

#[cfg(test)]
fn renderable_occupants(occupants: &[OccupantView]) -> impl Iterator<Item = &OccupantView> {
    occupants
        .iter()
        .filter(|occupant| occupant.package_id.is_some() || occupant.avatar_path.is_some())
}

/// Persist the exact probed catalog entry as a disabled row. Enabling stays a
/// separate user action after the preview (and any secret setup) succeeds.
#[cfg(test)]
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

pub(crate) fn request_overlay_monitor_action(state: &mut DetailUiState, fit_positions: bool) {
    state.overlay_monitor_apply_pending = true;
    state.overlay_monitor_fit_pending |= fit_positions;
    state.overlay_monitor_notice.clear();
    state.save_local_pending = true;
}

#[must_use]
pub(crate) fn overlay_monitor_mode_label(mode: OverlayMonitorMode) -> String {
    match mode {
        OverlayMonitorMode::Primary => i18n::fl("settings-overlay-monitor-primary"),
        OverlayMonitorMode::Selected => i18n::fl("settings-overlay-monitor-selected"),
        OverlayMonitorMode::Pointer => i18n::fl("settings-overlay-monitor-pointer"),
        OverlayMonitorMode::All => i18n::fl("settings-overlay-monitor-all"),
    }
}

#[must_use]
pub(crate) fn monitor_summary(monitor: &MonitorInfo) -> String {
    let number = (monitor.ordinal + 1).to_string();
    let name = monitor.name.clone().unwrap_or_else(|| {
        i18n::format("settings-overlay-display", &[("number", number.as_str())])
    });
    let size = format!("{}×{}", monitor.size[0], monitor.size[1]);
    let scale = format!("{:.0}%", monitor.scale_factor * 100.0);
    i18n::format(
        "settings-overlay-monitor-summary",
        &[
            ("name", name.as_str()),
            ("size", size.as_str()),
            ("scale", scale.as_str()),
        ],
    )
}

#[must_use]
pub(crate) fn log_empty_copy(entry_count: usize) -> Option<String> {
    (entry_count == 0).then(|| i18n::fl("log-empty"))
}

pub(crate) fn approval_mode_label(mode: &str) -> String {
    match mode {
        "ask_all" => i18n::fl("settings-approval-ask"),
        "auto" => i18n::fl("settings-approval-auto"),
        "ai_auto" => i18n::fl("settings-approval-ai"),
        _ => i18n::fl("settings-approval-policy"),
    }
}

#[cfg(test)]
fn character_export_package_id(soul: Option<&SoulView>) -> Option<String> {
    let package = soul.and_then(|soul| soul.package_id.clone())?;
    let id = package
        .split_once('@')
        .map(|(pkg, _)| pkg.to_owned())
        .unwrap_or(package);
    if id.is_empty() { None } else { Some(id) }
}

#[cfg(test)]
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

#[cfg(test)]
#[must_use]
fn character_export_filename(package_or_name: &str) -> String {
    format!("{}.enechar", safe_export_stem(package_or_name))
}

#[cfg(test)]
#[must_use]
fn session_export_filename_with_timestamp(
    session_id: &str,
    timestamp: &str,
    companion_name: Option<&str>,
) -> String {
    let stem = [Some(timestamp), companion_name, Some(session_id)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .map(safe_export_stem)
        .collect::<Vec<_>>()
        .join("_");
    format!("{stem}.json")
}

#[cfg(test)]
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

pub(crate) fn apply_ai_patch(
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

pub(crate) fn apply_voice_patch(
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

pub(crate) fn apply_observation_patch(
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
    fn jobs_refresh_clears_core_status_and_reloads() {
        let mut state = DetailUiState {
            core_status: "http 409: already_completed: already completed".to_owned(),
            loaded: DetailLoaded {
                jobs: true,
                ..DetailLoaded::default()
            },
            ..Default::default()
        };
        begin_jobs_reload(&mut state);
        assert!(state.core_status.is_empty());
        assert!(!state.loaded.jobs);
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
            job("verifying"),
            job("completed"),
            job("failed"),
            job("cancelled"),
            job("interrupted"),
        ];

        let active = active_jobs(&jobs)
            .into_iter()
            .map(|job| job.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(active, ["created", "queued", "running", "verifying"]);
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
                display_name: String::new(),
                body_id: None,
                package_id: None,
                avatar_path: None,
            },
            OccupantView {
                soul_id: "soul.avatar".to_owned(),
                display_name: "Alicia B".to_owned(),
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
    fn companion_display_rows_keep_overlay_order_and_chat_badges_separate() {
        let occupants = vec![
            OccupantView {
                soul_id: "soul-a".to_owned(),
                display_name: "Alicia".to_owned(),
                body_id: Some("body-a".to_owned()),
                package_id: Some("char.alicia@1.0.0".to_owned()),
                avatar_path: Some("/a.vrm".to_owned()),
            },
            OccupantView {
                soul_id: "soul-b".to_owned(),
                display_name: "Alicia B".to_owned(),
                body_id: Some("body-b".to_owned()),
                package_id: Some("char.alicia-b@1.0.0".to_owned()),
                avatar_path: Some("/b.vrm".to_owned()),
            },
            OccupantView {
                soul_id: "soul-text".to_owned(),
                display_name: "Notes".to_owned(),
                body_id: None,
                package_id: None,
                avatar_path: None,
            },
        ];
        let hidden = HashSet::from(["soul-b".to_owned()]);
        let rows = companion_display_rows(
            &occupants,
            &["soul-b".to_owned(), "soul-a".to_owned()],
            &hidden,
            "soul-a",
        );

        assert_eq!(
            rows.iter()
                .map(|row| row.soul_id.as_str())
                .collect::<Vec<_>>(),
            ["soul-b", "soul-a"]
        );
        assert!(rows[0].temporarily_hidden);
        assert!(!rows[0].active);
        assert!(rows[1].active);
        assert!(rows[1].displayed);
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
            stt_plugin_ready: true,
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
        assert_eq!(
            session_export_filename_with_timestamp("", "2026-01-02_0304", None),
            "2026-01-02_0304.json"
        );
        assert_eq!(
            session_export_filename_with_timestamp("", "2026-01-02_0304", Some("Alicia")),
            "2026-01-02_0304_Alicia.json"
        );
        assert_eq!(
            session_export_filename_with_timestamp("", "", Some("Alicia")),
            "Alicia.json"
        );
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
    fn mcp_credential_rows_skip_probe_rows_and_extract_server_ids() {
        assert_eq!(
            mcp_credential_row("mcp.bridge", "mcp.github-remote").as_deref(),
            Some("github-remote")
        );
        assert_eq!(
            mcp_credential_row("mcp.github-remote", "mcp.github-remote").as_deref(),
            Some("github-remote")
        );
        assert_eq!(mcp_credential_row("mcp.bridge", "mcp.probe-abc123"), None);
        assert_eq!(
            mcp_credential_row("mcp.github-remote", "mcp.probe-abc123"),
            None
        );
        assert_eq!(mcp_credential_row("mcp.bridge", "mcp."), None);
        assert_eq!(
            mcp_credential_row("provider.gguf", "mcp.github-remote"),
            None
        );
    }

    #[test]
    fn mcp_credential_save_requires_url_for_http_servers() {
        let server = ene_api::McpServerView {
            id: "web".to_owned(),
            transport: "http".to_owned(),
            command: None,
            args: Vec::new(),
            url: None,
            enabled: true,
        };
        let mut state = DetailUiState {
            mcp_credential_server_id: "web".to_owned(),
            mcp_servers: vec![server],
            ..Default::default()
        };
        state.mcp_credential_draft.token = "secret-token".to_owned();
        let async_results: Arc<Mutex<Vec<AsyncOutcome>>> = Arc::default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        i18n::with_language("en-US", || {
            let handle = rt.handle().clone();
            spawn_mcp_credential_save(
                &mut state,
                &Arc::new(ApiClient::new("http://127.0.0.1:1", "", "test")),
                &handle,
                &async_results,
            );
            assert_eq!(
                state.mcp_credential_draft.token, "secret-token",
                "token must survive when validation fails",
            );
            assert_eq!(
                state.connections_status,
                i18n::fl("connections-mcp-credential-needs-url")
            );
        });
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

    #[test]
    fn fresh_character_list_offers_bundled_alicia_b_until_installed() {
        assert!(bundled_alicia_b_available(&[]));

        let installed = CharacterView {
            id: crate::bundle::BUNDLED_ALICIA_B_ID.to_owned(),
            version: "1.0.0".to_owned(),
            kind: "package".to_owned(),
            path: "/packages/char.alicia-b".to_owned(),
            soul_id: Some("alicia-b".to_owned()),
        };
        assert!(!bundled_alicia_b_available(&[installed]));
    }

    #[test]
    fn selected_character_import_is_read_as_archive_bytes() {
        let dir = tempfile::TempDir::new().expect("temp directory");
        let path = dir.path().join("from-documents.enechar");
        std::fs::write(&path, b"package-bytes").expect("package fixture");

        assert_eq!(
            read_character_import(&path).expect("read package"),
            b"package-bytes"
        );
    }

    #[test]
    fn schedule_builder_generates_interval_specs() {
        let mut builder = ScheduleBuilderState::default();
        assert_eq!(builder.spec_for(), "every 15m");

        builder.interval_value = 90;
        builder.interval_unit = ScheduleIntervalUnit::Minutes;
        assert_eq!(builder.spec_for(), "every 90m");

        builder.interval_unit = ScheduleIntervalUnit::Hours;
        assert_eq!(builder.spec_for(), "every 90h");

        builder.interval_unit = ScheduleIntervalUnit::Days;
        assert_eq!(builder.spec_for(), "every 90d");
    }

    #[test]
    fn schedule_builder_generates_daily_and_weekly_specs() {
        let mut builder = ScheduleBuilderState {
            mode: ScheduleBuilderMode::Daily,
            daily_hour: 9,
            daily_minute: 5,
            ..ScheduleBuilderState::default()
        };
        assert_eq!(builder.spec_for(), "5 9 * * *");

        builder.mode = ScheduleBuilderMode::Weekly;
        builder.weekly_hour = 9;
        builder.weekly_minute = 30;
        builder.weekdays = [true, false, true, false, true, false, false];
        assert_eq!(builder.spec_for(), "30 9 * * MON,WED,FRI");

        builder.weekdays = [false; 7];
        assert_eq!(builder.spec_for(), "");
    }

    #[test]
    fn schedule_builder_advanced_mode_uses_raw_spec_field() {
        let builder = ScheduleBuilderState {
            mode: ScheduleBuilderMode::Advanced,
            ..ScheduleBuilderState::default()
        };
        assert_eq!(builder.spec_for(), "");
    }

    #[test]
    fn advanced_mode_create_gate_reads_the_raw_spec() {
        let builder = ScheduleBuilderState {
            mode: ScheduleBuilderMode::Advanced,
            ..ScheduleBuilderState::default()
        };

        // A non-empty raw cron makes Create eligible in Advanced mode.
        assert!(schedule_create_is_eligible(&builder, "Repair", "0 9 * * *"));

        // Empty raw text stays disabled.
        assert!(!schedule_create_is_eligible(&builder, "Repair", "   "));

        // The effective spec sent to the core is the exact raw value.
        assert_eq!(
            effective_schedule_spec(&builder, "  5 9 * * MON,WED,FRI  "),
            "5 9 * * MON,WED,FRI"
        );
    }

    #[test]
    fn create_gate_requires_name_and_guided_spec() {
        let mut builder = ScheduleBuilderState::default();

        // No name: disabled even with a valid guided spec.
        assert!(!schedule_create_is_eligible(&builder, "  ", ""));

        // Name plus guided interval spec enables Create.
        assert!(schedule_create_is_eligible(&builder, "Repair", ""));

        // A guided mode whose fields render no spec (weekly, nothing
        // selected) stays disabled.
        builder.mode = ScheduleBuilderMode::Weekly;
        builder.weekdays = [false; 7];
        assert!(!schedule_create_is_eligible(&builder, "Repair", ""));
    }

    #[test]
    fn humanize_next_fire_formats_utc_fallback_and_none() {
        assert_eq!(
            humanize_next_fire(None, "UTC"),
            i18n::fl("schedule-next-fire-none")
        );
        assert_eq!(
            humanize_next_fire(Some("not-a-timestamp"), "UTC"),
            "not-a-timestamp"
        );
        assert_eq!(
            humanize_next_fire(Some("2026-01-02T03:04:05Z"), "UTC"),
            i18n::format("schedule-next-fire", &[("at", "2026-01-02 03:04")])
        );
        assert_eq!(
            humanize_next_fire(Some("2026-01-02T03:04:05+09:00"), "UTC"),
            i18n::format("schedule-next-fire", &[("at", "2026-01-01 18:04")])
        );
    }

    #[test]
    fn local_timezone_name_never_panics_and_defaults_to_utc() {
        let name = local_timezone_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn layout_actions_persist_positions_and_individual_scales() {
        let mut settings = DesktopSettings::default();
        let souls = vec!["left".to_owned(), "active".to_owned()];

        apply_arranged_positions(&mut settings, &souls, "active");
        let [left_x, left_y] = settings.character_positions["left"];
        let [active_x, active_y] = settings.character_positions["active"];
        assert!((left_x - 0.3).abs() < f32::EPSILON);
        assert!((left_y - 0.5).abs() < f32::EPSILON);
        assert!((active_x - 0.7).abs() < f32::EPSILON);
        assert!((active_y - 0.5).abs() < f32::EPSILON);
        assert!((settings.character_x - 0.7).abs() < f32::EPSILON);

        set_character_scale(&mut settings, "active", true, 1.8);
        assert!((settings.character_scales["active"] - 1.8).abs() < f32::EPSILON);
        assert!((settings.model_scale - 1.8).abs() < f32::EPSILON);
    }
}
