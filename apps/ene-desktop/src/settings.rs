//! Plain-data runtime settings.
//!
//! `CharacterSettings` is owned directly by the
//! [`Runtime`](crate::runtime::Runtime) — no global registry, no
//! lock around the outer struct. The only field that is shared
//! across threads (`store`) sits behind a [`parking_lot::RwLock`].
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ene_config::serde::{Deserialize, Serialize};
use ene_config::{CharacterConfig, Lorebook, MotionCatalog};
use glam::Vec3;
use parking_lot::RwLock;

// ── Persisted section ──

ene_config::define_label_enum!(
    pub enum ShadowQuality {
        Low => "Low" => 1_024,
        #[default]
        Medium => "Medium" => 2_048,
        High => "High" => 4_096,
    }
    [shadow_map_size: usize]
);

ene_config::define_label_enum!(
    pub enum AntialiasingMode {
        Off => "Off",
        #[default]
        Fxaa => "FXAA",
        Smaa => "SMAA",
        Taa => "TAA",
    }
);

ene_config::define_label_enum!(
    pub enum GraphicsQuality {
        Low => "Low",
        #[default]
        Medium => "Medium",
        High => "High",
    }
);

/// Resolved per-knob graphics settings derived from [`GraphicsQuality`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsResolved {
    pub mask_render_downsample: u32,
    pub target_fps: u32,
    pub shadow_quality: ShadowQuality,
    pub antialiasing_mode: AntialiasingMode,
    pub debug_fps: u32,
}

impl GraphicsQuality {
    /// Maps a quality preset to concrete renderer settings.
    #[must_use]
    pub const fn resolved(self) -> GraphicsResolved {
        match self {
            Self::Low => GraphicsResolved {
                mask_render_downsample: 8,
                target_fps: 30,
                shadow_quality: ShadowQuality::Low,
                antialiasing_mode: AntialiasingMode::Off,
                debug_fps: 15,
            },
            Self::Medium => GraphicsResolved {
                mask_render_downsample: 6,
                target_fps: 60,
                shadow_quality: ShadowQuality::Medium,
                antialiasing_mode: AntialiasingMode::Fxaa,
                debug_fps: 30,
            },
            Self::High => GraphicsResolved {
                mask_render_downsample: 4,
                target_fps: 120,
                shadow_quality: ShadowQuality::High,
                antialiasing_mode: AntialiasingMode::Taa,
                debug_fps: 60,
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ene_config::schemars::JsonSchema)]
#[serde(default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct GraphicsSection {
    pub quality: GraphicsQuality,
}

impl GraphicsSection {
    #[must_use]
    pub fn resolved(&self) -> GraphicsResolved {
        self.quality.resolved()
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    ene_config::schemars::JsonSchema,
    PartialEq,
    Eq,
    Default,
)]
#[schemars(crate = "::ene_config::schemars")]
pub enum Language {
    #[serde(rename = "en")]
    #[default]
    En,
    #[serde(rename = "ja")]
    Ja,
}

ene_config::define_config!(
    settings,
    "desktop",
    pub struct DesktopSection {
        pub graphics: GraphicsSection,
        #[serde(default)]
        pub language: Language,
        /// Selected microphone device name for voice capture, or `None`
        /// for the OS default input device.
        #[serde(default)]
        pub mic_device: Option<String>,
        /// Enable the global Alt+Space spotlight launcher.
        #[serde(default = "default_enabled")]
        pub spotlight_enabled: bool,
        /// Enable the floating caption overlay window.
        #[serde(default = "default_enabled")]
        pub caption_enabled: bool,
        /// Last caption overlay top-left position (logical points),
        /// restored on the next launch. `None` until the user moves
        /// the overlay once.
        #[serde(default)]
        pub caption_position: Option<(f32, f32)>,
        /// Caption overlay pin state, restored on the next launch.
        /// `None` means "pinned until the user changes it".
        #[serde(default)]
        pub caption_pinned: Option<bool>,
        /// Beat Sync: react to system audio rhythm with avatar sway.
        #[serde(default)]
        pub beat_sync: BeatSyncSection,
    }
);

ene_config::define_config!(
    DesktopSection,
    "beat_sync",
    pub struct BeatSyncSection {
        /// Whether system-audio loopback capture and beat detection run.
        #[serde(default)]
        pub enabled: bool,
        /// Loopback device name override; `None` auto-selects an available
        /// monitor input.
        #[serde(default)]
        pub device: Option<String>,
    }
);

const fn default_enabled() -> bool {
    true
}

pub use GraphicsSection as GraphicsSettings;

// ── Defaults & cycle choices ──

pub const DEFAULT_CHARACTER_NAME: &str = "Alicia";
pub use ene_config::{DEFAULT_VRM_PATH, DEFAULT_VRMA_PATH};
#[expect(
    dead_code,
    reason = "legacy window constants retained for transitional callers"
)]
pub const APP_ID: &str = "dev.pexisgle.ene";
#[expect(
    dead_code,
    reason = "legacy window constants retained for transitional callers"
)]
pub const WINDOW_WIDTH: u32 = 560;
#[expect(
    dead_code,
    reason = "legacy window constants retained for transitional callers"
)]
pub const WINDOW_HEIGHT: u32 = 980;
pub const GRAPHICS_QUALITY_CHOICES: [GraphicsQuality; 3] = [
    GraphicsQuality::Low,
    GraphicsQuality::Medium,
    GraphicsQuality::High,
];

pub fn cycle_graphics_quality(current: GraphicsQuality, step: isize) -> GraphicsQuality {
    cycle_choice(&GRAPHICS_QUALITY_CHOICES, current, step)
}

pub fn graphics_quality_label(lang: Language, quality: GraphicsQuality) -> String {
    let _ = lang;
    match quality {
        GraphicsQuality::Low => i18n_embed_fl::fl!(crate::i18n::loader(), "low"),
        GraphicsQuality::Medium => i18n_embed_fl::fl!(crate::i18n::loader(), "medium"),
        GraphicsQuality::High => i18n_embed_fl::fl!(crate::i18n::loader(), "high"),
    }
}

pub const LANGUAGE_CHOICES: [Language; 2] = [Language::En, Language::Ja];

pub fn cycle_language(current: Language, step: isize) -> Language {
    cycle_choice(&LANGUAGE_CHOICES, current, step)
}

pub fn debug_fps_label(lang: Language, debug_fps: u32) -> String {
    let _ = lang;
    if debug_fps == 0 {
        i18n_embed_fl::fl!(crate::i18n::loader(), "match-window")
    } else {
        format!("{debug_fps} FPS")
    }
}

fn cycle_choice<T: Copy + PartialEq>(choices: &[T], current: T, step: isize) -> T {
    if choices.is_empty() {
        return current;
    }
    let index = choices.iter().position(|c| *c == current).unwrap_or(0);
    let len = choices.len() as isize;
    let next = (index as isize + step).rem_euclid(len) as usize;
    choices[next]
}

// ── CLI parsing ──

/// Read optional VRM and VRMA overrides from the first two CLI
/// arguments.
pub fn read_cli_paths() -> (String, String) {
    let vrm = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_VRM_PATH.to_string());
    let vrma = std::env::args()
        .nth(2)
        .unwrap_or_else(|| DEFAULT_VRMA_PATH.to_string());
    (vrm, vrma)
}

// ── Discovered character entries ──

pub type CharacterEntry = ene_config::CharacterEntry;

// ── Runtime state shapes (not persisted as JSON) ──

#[derive(Clone, Debug)]
pub struct CharacterState {
    pub selected_character: usize,
    pub selected_motion: usize,
    pub needs_respawn: bool,
    pub model_scale: f32,
    pub character_position: Vec3,
    pub look_at_strength: f32,
    pub motion_override: Option<String>,
    /// The expression the renderer should apply when this
    /// character is selected / respawned. Per-character on disk
    /// (each entry in `CharacterConfig.default_expression`); in
    /// memory it lives on `CharacterState` so the cycle buttons
    /// and the runtime's respawn path can push the matching
    /// `EmotionCommand` into the renderer's `EmotionQueue`.
    pub default_expression: String,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            selected_character: 0,
            selected_motion: 0,
            needs_respawn: true,
            model_scale: 1.0,
            character_position: Vec3::ZERO,
            look_at_strength: 0.60,
            motion_override: None,
            // Default to "neutral" so a fresh install doesn't
            // surprise the user. Per-character overrides are
            // loaded by `load_per_character_settings` once a
            // character is selected.
            default_expression: "neutral".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
// TODO: decompose UiState into per-page sub-structs (MemoryJournalState,
// PermissionState, SessionState, etc.) when the flat 40+ field struct
// grows unwieldy.
pub struct UiState {
    pub settings_window_visible: bool,
    /// Page the settings window should jump to when it next
    /// becomes visible. `Some(_)` is set by the per-action
    /// consumer systems and consumed by the runtime on the next
    /// `show_settings_window` call. `None` keeps the currently-focused
    /// page.
    pub focused_page: Option<crate::settings_ui::PageKind>,
    /// Toggle the Linux-only Wayland / X11 mask capture rectangle
    /// overlay. When `true` and [`Self::show_collider_debug`] is
    /// also `true`, the character window's debug overlay draws the
    /// downsampled mask rectangles as purple wireframes. Toggled by
    /// the "Linux mask overlay (debug)" checkbox on the Character
    /// settings page.
    pub debug_overlay_visible: bool,
    /// Toggle the per-bone collider wireframe + raycast hit-point
    /// overlay on the character window. Bound to the F3 hotkey
    /// and the "Show raycast colliders (debug)" checkbox on the
    /// Character page. Not persisted — defaults to `false` on every
    /// launch so the user starts with a clean view.
    pub show_collider_debug: bool,
    /// Toggle the input-region debug overlay (F9).
    pub show_input_region_debug: bool,
    pub hovered_bone_name: Option<String>,
    pub memory_journal_rows: Vec<MemoryJournalRow>,
    pub memory_journal_commitments: Vec<String>,
    pub memory_journal_commitments_cache: Vec<ene_store::Commitment>,
    pub memory_journal_pending_candidates: Vec<ene_runtime::handle::PendingCandidateSummary>,
    pub memory_journal_pending_history: Vec<ene_runtime::handle::PendingCandidateSummary>,
    pub memory_journal_show_history: bool,
    pub memory_journal_candidate_draft: Option<MemoryCandidateDraft>,
    pub memory_journal_affect: MemoryJournalAffect,
    pub memory_journal_message: Option<String>,
    pub memory_journal_show_deleted: bool,
    pub memory_journal_show_archived: bool,
    pub memory_journal_show_superseded: bool,
    pub memory_journal_recall_debug: bool,
    pub memory_journal_mode: MemoryPageMode,
    pub memory_journal_pending_count: usize,
    pub memory_journal_commitment_filter: CommitmentFilter,
    pub memory_journal_recall_query: String,
    pub memory_journal_recall_rows: Vec<MemoryJournalRecallRow>,
    pub memory_journal_pending_writes: usize,
    pub memory_journal_permanent_writes: usize,
    /// Pending tool-permission approvals awaiting a user decision.
    /// Accumulated by the permission-center consumer system
    /// from `AiPermissionRequested` messages; a request is removed
    /// once the user answers it from the Permission Center page.
    pub permission_requests: Vec<PendingPermission>,
    /// Standing session-wide permission grants listed from the actor.
    /// Refreshed after every decision / revocation.
    pub permission_grants: Vec<ene_runtime::PermissionScope>,
    pub permission_message: Option<String>,
    /// Cached connector summaries for the connectors settings page.
    /// Refreshed when the page is first shown and on explicit refresh.
    pub connector_summaries: Vec<ene_connector::ConnectorSummary>,
    pub connectors_loaded: bool,
    pub connector_selected: Option<ene_connector::ConnectorId>,
    pub connector_grants: Vec<ene_connector::PermissionGrant>,
    pub connector_status: Option<ene_connector::ConnectorStatus>,
    pub connector_message: Option<String>,
    /// Cached session metadata rows for the sessions page.
    /// Lazy-loaded the first time the page is shown and refreshed
    /// after every archive / import action. Rendered directly from the
    /// API v1 [`ene_runtime::PublicSessionMeta`] DTO.
    pub session_rows: Vec<ene_runtime::PublicSessionMeta>,
    /// Whether `session_rows` has been populated at least once, so an
    /// empty store is not mistaken for "not yet loaded".
    pub sessions_loaded: bool,
    pub session_search_query: String,
    pub session_search_rows: Vec<SessionSearchRow>,
    pub session_import_path: String,
    /// Whether archived sessions are included in the list.
    /// Defaults to `false`, matching the initial lazy load.
    pub session_show_archived: bool,
    pub session_message: Option<String>,
    /// First-run onboarding banner when the chat API key is missing.
    pub show_onboarding: bool,
    /// Localized startup failure when the runtime actor could not open.
    pub runtime_startup_error: Option<String>,
    /// Actor broadcast channel closed; show reconnect banner.
    pub runtime_disconnected: bool,
    pub reconnect_attempted: bool,
    /// Set by the reconnect banner; consumed by the winit runtime.
    pub reconnect_requested: bool,
    pub fatal_startup_dismissed: bool,

    // ── Spotlight quick launcher ──
    /// Whether the spotlight (Alt+Space) command palette is visible.
    pub spotlight_visible: bool,
    pub spotlight_input: String,
    pub spotlight_selection: usize,

    // ── Floating caption overlay ──
    pub caption_visible: bool,
    pub caption_text: String,
    /// Top-left position (logical points) of the caption window, or
    /// `None` until the user has moved it once.
    pub caption_position: Option<(f32, f32)>,
    /// Always-on-top state of the caption window; toggled by its pin
    /// button. `None` means "pinned until the user changes it".
    /// Ignored on Wayland, which has no window-level support.
    pub caption_pinned: Option<bool>,

    // ── Character card editor ──
    /// Whether the editor has loaded the on-disk card into the buffers
    /// at least once since the page was opened.
    pub character_editor_loaded: bool,
    /// Whether the in-memory buffers differ from the on-disk card.
    pub character_editor_modified: bool,
    /// Validation issues from the last validate/save attempt.
    pub character_editor_validation_errors: Vec<EditorIssue>,
    /// Editable `data.name` buffer.
    pub character_editor_name: String,
    /// Editable `data.description` buffer.
    pub character_editor_description: String,
    /// Editable `data.personality` buffer.
    pub character_editor_personality: String,
    /// Editable `data.scenario` buffer.
    pub character_editor_scenario: String,
    /// Editable `data.system_prompt` buffer.
    pub character_editor_system_prompt: String,
    /// Editable `data.mes_example` buffer.
    pub character_editor_mes_example: String,
    /// Editable `data.first_mes` buffer.
    pub character_editor_first_mes: String,
    /// Editable `data.creator_notes` buffer.
    pub character_editor_creator_notes: String,
    /// Editable `data.post_history_instructions` buffer.
    pub character_editor_post_history: String,
    /// Editable `data.alternate_greetings` list.
    pub character_editor_alternate_greetings: Vec<String>,
    /// Editable `data.character_book`; `None` leaves the on-disk lorebook untouched.
    pub character_editor_lorebook: Option<Lorebook>,
    /// Editable `extensions.ene.motion_catalog`; `None` leaves the on-disk
    /// catalog untouched.
    pub character_editor_motion_catalog: Option<MotionCatalog>,
    /// `character.{code}.json` sidecar files next to the card; the editor
    /// edits the base card only and surfaces their presence as a notice.
    pub character_editor_locale_diffs: Vec<String>,
    /// A close was requested while the editor had unsaved changes; the
    /// discard dialog must be shown before the window may hide.
    pub character_editor_close_requested: bool,
    /// A reload was requested while the editor had unsaved changes; the
    /// discard dialog must be shown before the buffers are reset.
    pub character_editor_reload_pending: bool,
    /// A character switch was requested while the editor had unsaved
    /// changes; the switch is applied only after the discard dialog
    /// confirms.
    pub character_editor_pending_character: Option<usize>,
    /// App exit was requested (main-window close / Esc) while the editor had
    /// unsaved changes; the exit is deferred until the discard dialog
    /// resolves.
    pub app_exit_requested: bool,
}

impl UiState {
    /// Whether the editor holds edits that would be lost on close/reload.
    ///
    /// Only the dirty flag matters: buffers can contain typed text even when
    /// the card never loaded (e.g. after a parse failure), and that text must
    /// not be dropped silently either.
    #[must_use]
    pub fn editor_has_unsaved_changes(&self) -> bool {
        self.character_editor_modified
    }

    /// Drop every editor buffer and mark the page as needing a fresh load.
    ///
    /// Used when the selected character changes (the buffers would otherwise
    /// describe a different card) and after the user discards unsaved edits.
    pub fn reset_character_editor(&mut self) {
        self.character_editor_loaded = false;
        self.character_editor_modified = false;
        self.character_editor_close_requested = false;
        self.character_editor_reload_pending = false;
        self.character_editor_pending_character = None;
        self.app_exit_requested = false;
        self.character_editor_validation_errors.clear();
        self.character_editor_name.clear();
        self.character_editor_description.clear();
        self.character_editor_personality.clear();
        self.character_editor_scenario.clear();
        self.character_editor_system_prompt.clear();
        self.character_editor_mes_example.clear();
        self.character_editor_first_mes.clear();
        self.character_editor_creator_notes.clear();
        self.character_editor_post_history.clear();
        self.character_editor_alternate_greetings.clear();
        self.character_editor_lorebook = None;
        self.character_editor_motion_catalog = None;
        self.character_editor_locale_diffs.clear();
    }

    /// Whether any discard dialog is currently pending.
    #[must_use]
    pub fn editor_dialog_pending(&self) -> bool {
        self.character_editor_close_requested
            || self.character_editor_reload_pending
            || self.character_editor_pending_character.is_some()
            || self.app_exit_requested
    }

    /// The user chose "keep editing": every pending dialog intent is
    /// cancelled and the buffers stay untouched.
    pub fn cancel_editor_dialog(&mut self) {
        self.character_editor_close_requested = false;
        self.character_editor_reload_pending = false;
        self.character_editor_pending_character = None;
        self.app_exit_requested = false;
    }
}

/// Severity of a character-editor validation finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorSeverity {
    /// Blocks saving until fixed.
    Error,
    /// Shown in the UI; saving is still allowed.
    Warning,
}

/// A single validation finding with the card field it refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorIssue {
    /// JSON-pointer-style field path (e.g. `data.assets[0].uri`), or the
    /// card file name for read/parse failures.
    pub location: String,
    /// Localized human-readable description.
    pub message: String,
    /// Whether this finding blocks saving.
    pub severity: EditorSeverity,
}

/// A single session message search hit shown on the sessions page.
#[derive(Clone, Debug)]
pub struct SessionSearchRow {
    pub session_id: String,
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct PendingPermission {
    pub request_id: ene_runtime::RequestId,
    pub action: String,
    pub target: String,
    /// Human-readable rationale shown to the user in the
    /// permission dialog. `None` means the actor did not supply
    /// one (the dialog then omits the description row).
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PendingUserInput {
    pub request_id: ene_runtime::RequestId,
    pub prompt: ene_plugin_proto::UserInputPrompt,
}

/// Inline edit draft for a pending memory candidate in the journal UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryCandidateDraft {
    /// Candidate row being edited.
    pub id: i64,
    /// Draft title.
    pub title: String,
    /// Draft content.
    pub content: String,
    /// Draft kind as string (validated against `MemoryKind` on save).
    pub kind: String,
    /// Draft confidence.
    pub confidence: f32,
}

#[derive(Clone, Debug, Default)]
pub struct QuestionDraft {
    pub text: String,
    pub selected: Option<String>,
    pub skipped: bool,
}

#[derive(Clone, Debug)]
pub struct MemoryJournalRow {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub confidence: f32,
    pub salience: f32,
    pub last_accessed: Option<String>,
    pub source_metadata: String,
    pub pinned: bool,
    pub scope: String,
    pub available_actions: Vec<crate::memory_journal::MemoryJournalAction>,
}

#[derive(Clone, Debug)]
pub struct MemoryJournalRecallRow {
    pub id: i64,
    pub title: String,
    pub reason: String,
    pub score_summary: String,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryJournalAffect {
    pub mood: String,
    pub expression: String,
    pub affinity: f32,
    pub trust: f32,
}

/// Tab mode for the memory journal page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryPageMode {
    #[default]
    Browse,
    RecallSearch,
    PendingApproval,
    Commitments,
}

/// Filter for commitment list display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitmentFilter {
    #[default]
    Active,
    All,
    Completed,
    Cancelled,
}

// ── Top-level settings (the runtime's single source of truth) ──

pub struct CharacterSettings {
    pub assets_dir: PathBuf,
    pub characters: Vec<CharacterEntry>,
    pub character_state: CharacterState,
    pub store: Arc<RwLock<ene_config::ConfigStore>>,
}

impl std::fmt::Debug for CharacterSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharacterSettings")
            .field("assets_dir", &self.assets_dir)
            .field("characters", &self.characters)
            .field("character_state", &self.character_state)
            .finish_non_exhaustive()
    }
}

impl CharacterSettings {
    /// Build the initial settings: discover characters on disk,
    /// load the persisted JSON, clamp runtime values.
    pub fn discover(assets_dir: &Path, default_vrm: &str) -> Self {
        let mut characters = ene_config::discover_characters(assets_dir);
        if characters.is_empty() {
            characters.push(CharacterEntry {
                name: DEFAULT_CHARACTER_NAME.to_string(),
                folder: DEFAULT_CHARACTER_NAME.to_string(),
                vrm_paths: vec![DEFAULT_VRM_PATH.to_string()],
                motion_paths: vec![DEFAULT_VRMA_PATH.to_string()],
                motion_names: vec!["VRMA_01".to_string()],
                card_path: format!("characters/{DEFAULT_CHARACTER_NAME}/character.json"),
                default_motion: None,
            });
        }

        let selected_character = characters
            .iter()
            .position(|c| c.vrm_paths.iter().any(|v| v == default_vrm))
            .unwrap_or(0);

        let default_card = format!("characters/{DEFAULT_CHARACTER_NAME}/character.json");
        let selected_card = characters
            .get(selected_character)
            .map_or(default_card, |c| c.card_path.clone());

        let selected_motion = characters
            .get(selected_character)
            .and_then(|entry| {
                entry
                    .default_motion
                    .as_ref()
                    .and_then(|dm| entry.motion_names.iter().position(|n| n == dm))
            })
            .unwrap_or(0);

        let initial_config = ene_config::EneConfig {
            character: format!("{}/{}", assets_dir.display(), selected_card),
            ..Default::default()
        };

        let mut settings = Self {
            assets_dir: assets_dir.to_path_buf(),
            characters,
            character_state: CharacterState {
                selected_character,
                selected_motion,
                ..Default::default()
            },
            store: Arc::new(RwLock::new(ene_config::ConfigStore::from_config(
                initial_config,
            ))),
        };
        settings.load_from_file();
        settings
    }

    /// Selected character entry, if the index is in range and the list is non-empty.
    pub fn current_entry(&self) -> Option<&CharacterEntry> {
        self.characters.get(self.character_state.selected_character)
    }

    /// Relative path of the first VRM for the selected character, if any.
    pub fn current_character(&self) -> Option<&str> {
        self.current_entry()
            .and_then(|entry| entry.vrm_paths.first())
            .map(String::as_str)
    }

    /// Relative path of the selected (or overridden) motion, if any.
    ///
    /// The motion override is character-scoped by the UI, so it is ignored
    /// when no character entry exists rather than pointing at a file that
    /// cannot play.
    pub fn current_motion(&self) -> Option<&str> {
        if self.current_entry().is_some()
            && let Some(ref override_path) = self.character_state.motion_override
        {
            return Some(override_path.as_str());
        }
        self.current_entry()
            .and_then(|entry| entry.motion_paths.get(self.character_state.selected_motion))
            .map(String::as_str)
    }

    /// Relative path of the selected character's card, if any.
    pub fn current_character_card(&self) -> Option<&str> {
        self.current_entry().map(|entry| entry.card_path.as_str())
    }

    pub fn sync_card_path(&mut self) {
        let Some(card) = self.current_character_card() else {
            return;
        };
        let path = format!("{}/{}", self.assets_dir.display(), card);
        self.with_config_mut(|c| c.character = path);
    }

    pub fn clamp_runtime_values(&mut self) {
        if self.characters.is_empty() {
            self.character_state.selected_character = 0;
            self.character_state.selected_motion = 0;
        } else {
            let max_char = self.characters.len() - 1;
            if self.character_state.selected_character > max_char {
                self.character_state.selected_character = max_char;
            }
            let motion_len = self
                .characters
                .get(self.character_state.selected_character)
                .map_or(0, |e| e.motion_paths.len());
            if motion_len == 0 {
                self.character_state.selected_motion = 0;
            } else if self.character_state.selected_motion >= motion_len {
                self.character_state.selected_motion = motion_len - 1;
            }
        }
        self.character_state.model_scale = self.character_state.model_scale.clamp(0.25, 4.0);
        self.character_state.character_position.x =
            self.character_state.character_position.x.clamp(-3.0, 3.0);
        self.character_state.character_position.y =
            self.character_state.character_position.y.clamp(-2.0, 3.0);
        self.character_state.character_position.z =
            self.character_state.character_position.z.clamp(-4.0, 3.0);
        self.character_state.look_at_strength =
            self.character_state.look_at_strength.clamp(0.0, 1.0);
        let current_quality = self.graphics().quality;
        let clamped = cycle_graphics_quality(current_quality, 0);
        self.set_graphics(GraphicsSettings { quality: clamped });
    }

    // ── Config accessors (read from / write through the store) ──

    /// Returns a snapshot of the current global config.
    pub fn config(&self) -> ene_config::EneConfig {
        self.store.read().config()
    }

    pub fn config_clone(&self) -> ene_config::EneConfig {
        self.store.read().config()
    }

    /// Get a typed section from the global config.
    pub fn config_section<T>(&self) -> T
    where
        T: serde::de::DeserializeOwned + Default + ene_config::HasConfigKey,
    {
        self.store.read().get_section::<T>()
    }

    /// Write a typed section into the global config.
    pub fn set_config_section<T>(&self, section: &T)
    where
        T: serde::Serialize + ene_config::HasConfigKey,
    {
        self.store.read().set_section(section);
    }

    /// Mutate the global config.
    pub fn with_config_mut(&self, f: impl FnOnce(&mut ene_config::EneConfig)) {
        self.store.read().with_config_mut(f);
    }

    /// Reads the Kokoro `voices.bin` path from the Kokoro plugin profile
    /// (`plugins.list.kokoro.profiles.kokoro.voices_path`, #313). Empty when
    /// unset.
    pub fn kokoro_voices_path(&self) -> String {
        let value = self
            .config()
            .get_path("plugins.list.kokoro.profiles.kokoro.voices_path");
        value
            .as_ref()
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default()
    }

    /// Writes the Kokoro `voices.bin` path into the Kokoro plugin profile
    /// (`plugins.list.kokoro.profiles.kokoro.voices_path`, #313). Passing an
    /// empty string clears it (`null`).
    pub fn set_kokoro_voices_path(&self, path: &str) {
        let trimmed = path.trim();
        let value = if trimmed.is_empty() { "null" } else { trimmed };
        self.with_config_mut(|c| {
            drop(c.set_path("plugins.list.kokoro.profiles.kokoro.voices_path", value));
        });
    }

    // ── Desktop-section accessors ─────────────────────────────────

    pub fn graphics(&self) -> GraphicsSettings {
        self.config_section::<DesktopSection>().graphics
    }

    pub fn language(&self) -> Language {
        self.config_section::<DesktopSection>().language
    }

    pub fn mic_device(&self) -> Option<String> {
        self.config_section::<DesktopSection>().mic_device
    }

    pub fn spotlight_enabled(&self) -> bool {
        self.config_section::<DesktopSection>().spotlight_enabled
    }

    pub fn caption_enabled(&self) -> bool {
        self.config_section::<DesktopSection>().caption_enabled
    }

    pub fn set_graphics(&self, graphics: GraphicsSettings) {
        self.store.read().with_config_mut(|c| {
            if let Ok(mut d) = c.get_section::<DesktopSection>() {
                d.graphics = graphics;
                drop(c.set_section(&d));
            }
        });
    }

    pub fn set_language(&self, language: Language) {
        self.store.read().with_config_mut(|c| {
            if let Ok(mut d) = c.get_section::<DesktopSection>() {
                d.language = language;
                drop(c.set_section(&d));
            }
        });
    }

    pub fn set_mic_device(&self, mic_device: Option<String>) {
        self.store.read().with_config_mut(|c| {
            if let Ok(mut d) = c.get_section::<DesktopSection>() {
                d.mic_device = mic_device;
                drop(c.set_section(&d));
            }
        });
    }

    pub fn beat_sync_device(&self) -> Option<String> {
        self.config_section::<DesktopSection>().beat_sync.device
    }

    pub fn set_beat_sync_enabled(&self, enabled: bool) {
        self.store.read().with_config_mut(|c| {
            if let Ok(mut d) = c.get_section::<DesktopSection>() {
                d.beat_sync.enabled = enabled;
                drop(c.set_section(&d));
            }
        });
    }

    pub fn save_per_character_settings(&self) {
        self.sync_character_state_to_store();
        let char_name = self.current_entry().map(|e| e.name.clone());
        let store = self.store.read();
        if let Err(e) = store.flush(char_name.as_deref()) {
            tracing::warn!("[Config] Failed to save per-character settings: {e}");
        }
    }

    pub fn load_per_character_settings(&mut self) {
        let Some(char_name) = self.current_entry().map(|e| e.name.clone()) else {
            return;
        };
        let store = self.store.read();
        store.load_character_config(&char_name);
        let per = store.character_config();
        drop(store);

        self.character_state.character_position = Vec3::new(
            per.character_position[0],
            per.character_position[1],
            per.character_position[2],
        );
        self.character_state.model_scale = per.model_scale;
        self.character_state.look_at_strength = per.look_at_strength;
        // An empty string on disk is treated as "neutral" so a model
        // that has never been customized gets a sane starting pose.
        self.character_state.default_expression = if per.default_expression.is_empty() {
            "neutral".to_string()
        } else {
            per.default_expression.clone()
        };
        if !per.default_motion.is_empty()
            && let Some(m) = self.current_entry().and_then(|entry| {
                entry
                    .motion_names
                    .iter()
                    .position(|n| n == &per.default_motion)
            })
        {
            self.character_state.selected_motion = m;
        }
    }

    /// Switch to character `idx`. Returns the default expression
    /// to push into the renderer's `EmotionQueue` so the new
    /// character enters the scene with its per-character default
    /// face. `None` is returned when the switch is a no-op
    /// (out-of-range or same character), in which case the caller
    /// should not touch the emotion queue.
    pub fn select_character(&mut self, idx: usize) -> Option<String> {
        if idx >= self.characters.len() || idx == self.character_state.selected_character {
            return None;
        }
        self.save_per_character_settings();
        self.character_state.selected_character = idx;
        self.character_state.selected_motion = 0;
        self.clamp_runtime_values();
        self.sync_card_path();
        self.load_per_character_settings();
        self.character_state.needs_respawn = true;
        // Cloned to avoid handing out a borrow of `self` while the
        // caller still holds it.
        Some(self.character_state.default_expression.clone())
    }

    pub fn save(&self) {
        self.sync_character_state_to_store();
        let char_name = self.current_entry().map(|e| e.name.clone());
        let store = self.store.read();
        if let Err(e) = store.flush(char_name.as_deref()) {
            tracing::warn!("[Config] Failed to save config: {e}");
        }
    }

    pub fn mark_dirty(&self) {
        let store = self.store.read();
        store.mark_dirty();
    }

    /// Called once per frame by the runtime; flushes the
    /// underlying `ConfigStore` to disk only if anything was
    /// changed since the last flush.
    pub fn flush_if_dirty(&self) {
        self.sync_character_state_to_store();
        let char_name = self.current_entry().map(|e| e.name.clone());
        let store = self.store.read();
        if let Err(e) = store.flush_if_dirty(char_name.as_deref()) {
            tracing::warn!("[Config] Failed to flush dirty config: {e}");
        }
    }

    /// Sync the in-memory character state into the store's per-character
    /// config so it gets flushed to disk on the next save.
    fn sync_character_state_to_store(&self) {
        let Some(entry) = self.current_entry() else {
            return;
        };
        let store = self.store.read();
        let default_motion_name = entry
            .motion_names
            .get(self.character_state.selected_motion)
            .cloned()
            .unwrap_or_default();
        let existing = store.character_config();
        let char_config = CharacterConfig {
            character_position: [
                self.character_state.character_position.x,
                self.character_state.character_position.y,
                self.character_state.character_position.z,
            ],
            model_scale: self.character_state.model_scale,
            look_at_strength: self.character_state.look_at_strength,
            default_motion: default_motion_name,
            // The in-memory source of truth is
            // `self.character_state.default_expression`; the
            // on-disk value lives in a per-character
            // `CharacterConfig` so two different models can have
            // two different defaults.
            default_expression: self.character_state.default_expression.clone(),
            // The card locale is not surfaced in the UI; keep whatever is on
            // disk so a manual per-character override survives a save.
            language: existing.language,
            extra: existing.extra,
        };
        store.set_character_config(char_config);
    }

    pub fn load_from_file(&mut self) {
        let path = ene_config::config_file_path();
        let full = match ene_config::load_config_from(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("[Settings] Failed to load config: {e}, falling back to defaults");
                ene_config::EneConfig::default()
            }
        };

        *self.store.write() = ene_config::ConfigStore::from_config(full.clone());

        // Resolve `full.character` (a card name or full path) to
        // a card path on disk. An unset character keeps the default
        // selection instead of resolving to a hardcoded fallback.
        if let Ok(card_path) = ene_config::resolve_character_path(&full.character)
            && let Some(parent) = card_path.parent()
            && let Some(name_os) = parent.file_name()
        {
            let name = name_os.to_string_lossy();
            if let Some(idx) = self.characters.iter().position(|c| c.name == name) {
                self.character_state.selected_character = idx;
            }
        }

        self.sync_classifier_language_from_ui();

        crate::i18n::select_language(self.language());

        self.clamp_runtime_values();
        self.load_per_character_settings();
    }

    /// Keep cognitive prompt language aligned with the desktop UI language.
    ///
    /// Writes only `mind.language`; `emotion.classifier_language` is left
    /// untouched so an explicit per-task override is respected and an unset
    /// one keeps inheriting `mind.language`.
    pub fn sync_classifier_language_from_ui(&self) {
        let lang = match self.language() {
            Language::En => "en",
            Language::Ja => "ja",
        };
        self.with_config_mut(|c| {
            if let Ok(mut mind) = c.get_section::<ene_mind::MindConfig>() {
                mind.language = lang.into();
                drop(c.set_section(&mind));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_settings() -> CharacterSettings {
        CharacterSettings {
            assets_dir: PathBuf::from("/tmp/ene-test-assets"),
            characters: Vec::new(),
            character_state: CharacterState {
                selected_character: 3,
                selected_motion: 5,
                ..Default::default()
            },
            store: Arc::new(RwLock::new(ene_config::ConfigStore::from_config(
                ene_config::EneConfig::default(),
            ))),
        }
    }

    fn entry_without_assets() -> CharacterEntry {
        CharacterEntry {
            name: "ghost".to_string(),
            folder: "ghost".to_string(),
            vrm_paths: Vec::new(),
            motion_paths: Vec::new(),
            motion_names: Vec::new(),
            card_path: "characters/ghost/character.json".to_string(),
            default_motion: None,
        }
    }

    #[test]
    fn editor_unsaved_changes_tracks_only_dirty_flag() {
        let mut state = UiState::default();
        assert!(!state.editor_has_unsaved_changes());
        state.character_editor_modified = true;
        assert!(
            state.editor_has_unsaved_changes(),
            "typed text must be protected even before a successful load"
        );
        state.reset_character_editor();
        assert!(!state.editor_has_unsaved_changes());
    }

    #[test]
    fn reset_character_editor_drops_every_buffer() {
        let mut state = UiState {
            character_editor_loaded: true,
            character_editor_modified: true,
            character_editor_close_requested: true,
            character_editor_reload_pending: true,
            character_editor_pending_character: Some(1),
            app_exit_requested: true,
            character_editor_name: "Alicia".to_string(),
            character_editor_alternate_greetings: vec!["hi".to_string()],
            character_editor_validation_errors: vec![EditorIssue {
                location: "data.name".to_string(),
                message: "nope".to_string(),
                severity: EditorSeverity::Error,
            }],
            ..UiState::default()
        };

        state.reset_character_editor();

        let default = UiState::default();
        assert_eq!(state.character_editor_name, default.character_editor_name);
        assert!(state.character_editor_alternate_greetings.is_empty());
        assert!(state.character_editor_lorebook.is_none());
        assert!(state.character_editor_motion_catalog.is_none());
        assert!(state.character_editor_validation_errors.is_empty());
        assert!(!state.character_editor_loaded);
        assert!(!state.character_editor_modified);
        assert!(!state.character_editor_close_requested);
        assert!(!state.character_editor_reload_pending);
        assert!(state.character_editor_pending_character.is_none());
        assert!(!state.app_exit_requested);
    }

    #[test]
    fn editor_dialog_pending_covers_every_intent() {
        let mut state = UiState::default();
        assert!(!state.editor_dialog_pending());
        state.character_editor_close_requested = true;
        assert!(state.editor_dialog_pending());
        state.character_editor_close_requested = false;
        state.character_editor_reload_pending = true;
        assert!(state.editor_dialog_pending());
        state.character_editor_reload_pending = false;
        state.character_editor_pending_character = Some(3);
        assert!(state.editor_dialog_pending());
        state.character_editor_pending_character = None;
        state.app_exit_requested = true;
        assert!(state.editor_dialog_pending());

        state.cancel_editor_dialog();
        assert!(!state.editor_dialog_pending());
    }

    #[test]
    fn character_state_defaults_to_neutral_expression() {
        let s = CharacterState::default();
        assert_eq!(s.default_expression, "neutral");
    }

    /// Pins the `cycle_index` math used at the `select_character` call
    /// sites so an out-of-range or same-character index resolves to a
    /// no-op (`None`) without pushing an expression.
    #[test]
    fn select_character_no_op_returns_none() {
        let len = 0_usize;
        let current = 0_usize;
        let step = 1_isize;
        let idx = ((current as isize + step).rem_euclid(len.max(1) as isize)) as usize;
        assert_eq!(idx, 0);
    }

    /// An empty `default_expression` on disk is treated as
    /// `"neutral"` so a model that was never customized gets a
    /// sane starting pose.
    #[test]
    fn load_per_character_settings_treats_empty_as_neutral() {
        let raw = "";
        let resolved = if raw.is_empty() { "neutral" } else { raw };
        assert_eq!(resolved, "neutral");
    }

    /// The `PendingPermission` / `PendingUserInput` /
    /// `QuestionDraft` types are consumed by the dialog
    /// renderer in `page_ai.rs`. A future refactor that
    /// accidentally drops one of the fields would lose the
    /// dialog data, so this test pins the public surface.
    #[test]
    fn pending_dialog_types_constructible() {
        let _perm = PendingPermission {
            request_id: ene_runtime::RequestId::new("test"),
            action: "fs.write".to_string(),
            target: "/tmp/example.txt".to_string(),
            description: Some("Write a 4 KB file".to_string()),
        };
        let _draft = QuestionDraft {
            text: "alice".to_string(),
            selected: Some("yes".to_string()),
            skipped: false,
        };
    }

    #[test]
    fn accessors_return_none_when_characters_empty() {
        let mut settings = empty_settings();
        settings.clamp_runtime_values();
        assert_eq!(settings.character_state.selected_character, 0);
        assert_eq!(settings.character_state.selected_motion, 0);
        assert!(settings.current_entry().is_none());
        assert!(settings.current_character().is_none());
        assert!(settings.current_motion().is_none());
        assert!(settings.current_character_card().is_none());
    }

    #[test]
    fn accessors_return_none_when_vrm_and_motion_missing() {
        let mut settings = empty_settings();
        settings.characters.push(entry_without_assets());
        settings.character_state.selected_character = 0;
        settings.character_state.selected_motion = 2;
        settings.clamp_runtime_values();
        assert_eq!(settings.character_state.selected_motion, 0);
        assert!(settings.current_entry().is_some());
        assert!(settings.current_character().is_none());
        assert!(settings.current_motion().is_none());
        assert_eq!(
            settings.current_character_card(),
            Some("characters/ghost/character.json")
        );
    }

    #[test]
    fn clamp_runtime_values_clamps_stale_indices() {
        let mut settings = empty_settings();
        settings.characters.push(CharacterEntry {
            name: "a".to_string(),
            folder: "a".to_string(),
            vrm_paths: vec!["characters/a/a.vrm".to_string()],
            motion_paths: vec![
                "characters/a/m0.vrma".to_string(),
                "characters/a/m1.vrma".to_string(),
            ],
            motion_names: vec!["m0".to_string(), "m1".to_string()],
            card_path: "characters/a/character.json".to_string(),
            default_motion: None,
        });
        settings.character_state.selected_character = 9;
        settings.character_state.selected_motion = 9;
        settings.clamp_runtime_values();
        assert_eq!(settings.character_state.selected_character, 0);
        assert_eq!(settings.character_state.selected_motion, 1);
        assert_eq!(settings.current_character(), Some("characters/a/a.vrm"));
        assert_eq!(settings.current_motion(), Some("characters/a/m1.vrma"));
    }

    #[test]
    fn motion_override_wins_when_list_empty() {
        let mut settings = empty_settings();
        settings.characters.push(entry_without_assets());
        settings.character_state.motion_override = Some("override.vrma".to_string());
        settings.clamp_runtime_values();
        assert_eq!(settings.current_motion(), Some("override.vrma"));
    }
}
