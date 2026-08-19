//! Settings UI — the tabbed settings window.
//!
//! The runtime owns a single [`SettingsUi`] per `UiWindow`. Each
//! frame the `UiWindow` calls [`SettingsUi::render`] with the live
//! `&mut CharacterSettings` and the `Arc<CoreSession>`.
pub mod apply;
pub mod artifact_card;
pub mod cloud_models;
pub mod components;
pub mod draft;
pub mod input;
pub mod page_accessibility;
pub mod page_advanced;
pub mod page_ai;
pub mod page_approvals;
pub mod page_character;
pub mod page_character_editor;
pub mod page_connectors;
pub mod page_debug;
pub mod page_engines;
pub mod page_features;
pub mod page_graphics;
pub mod page_memory;
pub mod page_memory_ledger;
pub mod page_overview;
pub mod page_permissions;
pub mod page_plugins;
pub mod page_schedules;
pub mod page_sessions;
pub mod page_voice;
pub mod provider_assets;
pub mod provider_form;
pub mod schema_form;
pub mod widgets;

pub use components::section_title;
pub use input::SettingsInputState;

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::character_state::{AnimationControl, EmotionQueue};
use crate::core_session::CoreSession;
use crate::settings::{CharacterSettings, Language};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use components::NARROW_NAV_THRESHOLD;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    #[default]
    /// Overview: needs-config, health issues, restart-pending, credentials.
    Overview,
    /// General: display, language, theme, spotlight, captions, hotkeys,
    /// accessibility.
    General,
    Character,
    /// Character Card (`CCv3`) editor.
    CharacterEditor,
    Ai,
    Voice,
    Features,
    Memory,
    /// Management view: memory journal + ledger merged as tabs.
    Memories,
    Permissions,
    Approvals,
    Connectors,
    Sessions,
    /// Scheduled tool runs: CRUD, next run, history, pending confirmations.
    Schedules,
    /// Plugin center: launch profile and running fibers.
    Plugins,
    /// Local inference engines: sidecars and provider bindings.
    Engines,
    /// Generic schema leaf editor for everything without a dedicated page.
    Advanced,
    /// Diagnostics: runtime/AI/voice/plugin health and debug overlays.
    Diagnostics,
}

/// Navigation grouping shown in the settings sidebar: user-facing settings
/// vs management/operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCategory {
    Settings,
    Management,
}

impl PageCategory {
    pub const ALL: [PageCategory; 2] = [PageCategory::Settings, PageCategory::Management];

    pub fn label(self) -> String {
        let key = match self {
            PageCategory::Settings => "page-category-settings",
            PageCategory::Management => "page-category-management",
        };
        crate::i18n::loader().get(key)
    }
}

/// Static per-page metadata: category, localized title/description keys,
/// search aliases, and the searchable section ids rendered by the page.
pub struct PageMeta {
    pub category: PageCategory,
    pub title_key: &'static str,
    pub description_key: &'static str,
    pub aliases: &'static [&'static str],
    pub sections: &'static [&'static str],
    pub section_aliases: &'static [SectionAlias],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionAlias {
    pub alias: &'static str,
    pub section: &'static str,
}

impl PageKind {
    pub const ALL: [PageKind; 18] = [
        PageKind::Overview,
        PageKind::General,
        PageKind::Character,
        PageKind::CharacterEditor,
        PageKind::Ai,
        PageKind::Voice,
        PageKind::Features,
        PageKind::Memory,
        PageKind::Memories,
        PageKind::Sessions,
        PageKind::Permissions,
        PageKind::Approvals,
        PageKind::Connectors,
        PageKind::Schedules,
        PageKind::Plugins,
        PageKind::Engines,
        PageKind::Advanced,
        PageKind::Diagnostics,
    ];

    pub fn meta(self) -> &'static PageMeta {
        match self {
            PageKind::Overview => &PageMeta {
                category: PageCategory::Settings,
                title_key: "page-overview",
                description_key: "page-overview-description",
                aliases: &["home", "start", "setup", "credential", "restart"],
                sections: &[
                    "overview-needs-config",
                    "overview-issues",
                    "overview-restart-pending",
                    "overview-credentials",
                ],
                section_aliases: &[],
            },
            PageKind::General => &PageMeta {
                category: PageCategory::Settings,
                title_key: "general",
                description_key: "page-general-description",
                aliases: &[
                    "graphics",
                    "display",
                    "quality",
                    "language",
                    "theme",
                    "dark",
                    "light",
                    "spotlight",
                    "caption",
                    "overlay",
                    "subtitle",
                    "accessibility",
                ],
                sections: &[
                    "graphics-quality",
                    "graphics-language",
                    "graphics-theme",
                    "accessibility-overlays",
                ],
                section_aliases: &[],
            },
            PageKind::Character => &PageMeta {
                category: PageCategory::Settings,
                title_key: "character-and-user",
                description_key: "page-character-description",
                aliases: &[
                    "vrm",
                    "motion",
                    "expression",
                    "animation",
                    "user",
                    "persona",
                    "user-name",
                    "name",
                ],
                sections: &[
                    "character-model",
                    "character-transform",
                    "character-expressions",
                ],
                section_aliases: &[],
            },
            PageKind::CharacterEditor => &PageMeta {
                category: PageCategory::Management,
                title_key: "character-card-editor",
                description_key: "page-character-card-description",
                aliases: &["card", "ccv3", "lorebook", "greeting"],
                sections: &[
                    "editor-identity",
                    "editor-personality",
                    "editor-scenario",
                    "editor-greetings",
                    "editor-memory",
                    "editor-lorebook",
                    "editor-motions",
                ],
                section_aliases: &[],
            },
            PageKind::Ai => &PageMeta {
                category: PageCategory::Settings,
                title_key: "ai-models",
                description_key: "page-ai-description",
                aliases: &[
                    "openai",
                    "api",
                    "chat",
                    "embedding",
                    "provider",
                    "model",
                    "models",
                    "anthropic",
                    "claude",
                    "gguf",
                    "local",
                    "classifier",
                    "proactive",
                ],
                sections: &["ai-conversation", "ai-advanced"],
                section_aliases: &[],
            },
            PageKind::Voice => &PageMeta {
                category: PageCategory::Settings,
                title_key: "voice-audio",
                description_key: "page-voice-description",
                aliases: &["tts", "stt", "vad", "speech", "mic", "microphone"],
                sections: &["voice-tts", "voice-stt", "voice-mic"],
                section_aliases: &[
                    SectionAlias {
                        alias: "tts",
                        section: "voice-tts",
                    },
                    SectionAlias {
                        alias: "stt",
                        section: "voice-stt",
                    },
                    SectionAlias {
                        alias: "vad",
                        section: "voice-mic",
                    },
                    SectionAlias {
                        alias: "mic",
                        section: "voice-mic",
                    },
                    SectionAlias {
                        alias: "microphone",
                        section: "voice-mic",
                    },
                ],
            },
            PageKind::Features => &PageMeta {
                category: PageCategory::Settings,
                title_key: "behavior",
                description_key: "page-features-description",
                aliases: &[
                    "proactive",
                    "mind",
                    "emotion",
                    "quiet",
                    "privacy",
                    "session",
                ],
                sections: &["features-mind"],
                section_aliases: &[],
            },
            PageKind::Memory => &PageMeta {
                category: PageCategory::Settings,
                title_key: "memory-storage",
                description_key: "page-memory-storage-description",
                aliases: &[
                    "memory",
                    "store",
                    "backup",
                    "integrity",
                    "approval",
                    "limit",
                    "in-memory",
                ],
                sections: &["memory-config", "memory-approval", "memory-limits"],
                section_aliases: &[],
            },
            PageKind::Memories => &PageMeta {
                category: PageCategory::Management,
                title_key: "memories",
                description_key: "page-memories-description",
                aliases: &[
                    "journal",
                    "ledger",
                    "recall",
                    "commitment",
                    "browse",
                    "delete",
                    "edit",
                ],
                sections: &[
                    "memory-browse",
                    "memory-recall",
                    "memory-pending",
                    "memory-commitments",
                    "ledger-browse",
                    "ledger-commitments",
                ],
                section_aliases: &[],
            },
            PageKind::Permissions => &PageMeta {
                category: PageCategory::Management,
                title_key: "permissions",
                description_key: "page-permissions-description",
                aliases: &["tools", "approval", "grant", "revoke"],
                sections: &["permissions-pending", "permissions-grants"],
                section_aliases: &[],
            },
            PageKind::Approvals => &PageMeta {
                category: PageCategory::Settings,
                title_key: "security-downloads",
                description_key: "page-approvals-description",
                aliases: &[
                    "plugin",
                    "policy",
                    "sandbox",
                    "broker",
                    "approval",
                    "audit",
                    "download",
                    "artifact",
                    "publisher",
                    "emergency",
                ],
                sections: &["approvals-policy"],
                section_aliases: &[],
            },
            PageKind::Connectors => &PageMeta {
                category: PageCategory::Management,
                title_key: "connectors",
                description_key: "page-connectors-description",
                aliases: &[
                    "accounts", "service", "oauth", "mcp", "git", "browser", "calendar",
                ],
                sections: &["connectors-list", "connectors-detail"],
                section_aliases: &[],
            },
            PageKind::Sessions => &PageMeta {
                category: PageCategory::Management,
                title_key: "sessions",
                description_key: "page-sessions-description",
                aliases: &["archive", "export", "import", "history"],
                sections: &["sessions-list", "sessions-search", "sessions-import"],
                section_aliases: &[],
            },
            PageKind::Schedules => &PageMeta {
                category: PageCategory::Management,
                title_key: "schedules",
                description_key: "page-schedules-description",
                aliases: &["cron", "timer", "scheduled", "repeat"],
                sections: &[
                    "schedules-list",
                    "schedules-history",
                    "schedules-pending",
                    "schedules-add",
                ],
                section_aliases: &[],
            },
            PageKind::Plugins => &PageMeta {
                category: PageCategory::Settings,
                title_key: "tools-and-plugins",
                description_key: "page-tools-and-plugins-description",
                aliases: &["tool", "provider", "tools", "sandbox", "plugin", "profile"],
                sections: &["plugins-profile", "plugins-list"],
                section_aliases: &[],
            },
            PageKind::Engines => &PageMeta {
                category: PageCategory::Management,
                title_key: "engines",
                description_key: "page-engines-description",
                aliases: &["sidecar", "engine", "inference", "llama-server"],
                sections: &["engines-plugins", "engines-list"],
                section_aliases: &[],
            },
            PageKind::Advanced => &PageMeta {
                category: PageCategory::Settings,
                title_key: "advanced",
                description_key: "page-advanced-description",
                aliases: &[
                    "json", "schema", "raw", "config", "expert", "hidden", "advanced",
                ],
                sections: &["advanced-sections"],
                section_aliases: &[],
            },
            PageKind::Diagnostics => &PageMeta {
                category: PageCategory::Management,
                title_key: "diagnostics",
                description_key: "page-debug-description",
                aliases: &[
                    "overlay", "fps", "mask", "collider", "debug", "health", "pipeline",
                ],
                sections: &["debug-overlays", "debug-pipeline"],
                section_aliases: &[],
            },
        }
    }

    pub fn title(self) -> String {
        crate::i18n::loader().get(self.meta().title_key)
    }

    pub fn description(self) -> String {
        crate::i18n::loader().get(self.meta().description_key)
    }
}

/// One search result: a page, or a section within a page.
#[derive(Clone, Debug)]
pub struct SearchHit {
    pub page: PageKind,
    pub section: Option<&'static str>,
    pub title: String,
    pub detail: String,
    /// For schema-leaf hits: the dotted config path used to pre-filter the
    /// Advanced page.
    pub filter: Option<String>,
    /// For plugin/action hits: the plugin card to open and focus.
    pub plugin_focus: Option<String>,
    rank: u8,
}

/// Outcome of the last draft apply, shown as a banner in the window chrome.
#[derive(Debug, Clone)]
pub struct ApplyFeedback {
    pub revision: u64,
    pub ok: bool,
    pub impact: apply::SettingsImpact,
    /// Section keys the runtime actually wrote (display detail).
    pub applied_sections: Vec<String>,
    pub message: Option<String>,
}

/// Ranked page/section search over localized titles, descriptions,
/// technical aliases (TTS/STT/VAD), section names, and configured plugin
/// names (which land on the plugin center page).
fn compute_search(query: &str, plugin_names: &[String]) -> Vec<SearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for page in PageKind::ALL {
        let meta = page.meta();
        let title = page.title();
        let title_lower = title.to_lowercase();
        let page_rank = if title_lower.contains(&query) {
            Some(0)
        } else if meta
            .aliases
            .iter()
            .any(|alias| alias.to_lowercase().contains(&query))
        {
            Some(2)
        } else if page.description().to_lowercase().contains(&query) {
            Some(3)
        } else {
            None
        };
        if let Some(rank) = page_rank {
            hits.push(SearchHit {
                page,
                section: None,
                title: title.clone(),
                detail: page.description(),
                filter: None,
                plugin_focus: None,
                rank,
            });
        }
        for section in meta.sections {
            let Some(section_title) = section_title(section) else {
                continue;
            };
            let title_match = section_title.to_lowercase().contains(&query);
            let alias_match = meta.section_aliases.iter().any(|alias| {
                alias.section == *section && alias.alias.to_lowercase().contains(&query)
            });
            if title_match || alias_match {
                hits.push(SearchHit {
                    page,
                    section: Some(section),
                    title: section_title,
                    detail: title.clone(),
                    filter: None,
                    plugin_focus: None,
                    rank: 1,
                });
            }
        }
    }
    for name in plugin_names {
        if name.to_lowercase().contains(&query) {
            hits.push(SearchHit {
                page: PageKind::Plugins,
                section: None,
                title: name.clone(),
                detail: i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "page-tools-and-plugins-description"
                ),
                filter: None,
                plugin_focus: Some(name.clone()),
                rank: 4,
            });
        }
    }
    // Selecting one navigates to the Advanced page with the filter
    // pre-applied.
    for (section_key, entry) in
        ene_config::config::registered_schemas_for(ene_config::ConfigTarget::Settings)
    {
        let Ok(schema) = serde_json::to_value(&entry.schema) else {
            continue;
        };
        let mut leaves = Vec::new();
        collect_schema_leaves(&schema, &section_key, &mut leaves);
        for (path, title, description) in leaves {
            let path_lower = path.to_lowercase();
            let title_lower = title.to_lowercase();
            if path_lower.contains(&query)
                || title_lower.contains(&query)
                || description.to_lowercase().contains(&query)
            {
                hits.push(SearchHit {
                    page: PageKind::Advanced,
                    section: Some("advanced-sections"),
                    title: path,
                    detail: title,
                    filter: Some(path_lower),
                    plugin_focus: None,
                    rank: 5,
                });
            }
        }
    }
    hits.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.title.cmp(&b.title)));
    hits.truncate(16);
    hits
}

fn collect_schema_leaves(
    schema: &serde_json::Value,
    prefix: &str,
    out: &mut Vec<(String, String, String)>,
) {
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for (name, property_schema) in properties {
            let child = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            let title = property_schema
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(name);
            let description = property_schema
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let has_children = property_schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|properties| !properties.is_empty());
            if has_children {
                collect_schema_leaves(property_schema, &child, out);
            } else {
                out.push((child.clone(), title.to_string(), description.to_string()));
            }
        }
    }
    if let Some(items) = schema.get("items") {
        collect_schema_leaves(items, prefix, out);
    }
}

const fn use_compact_navigation(width: f32) -> bool {
    width < NARROW_NAV_THRESHOLD
}

/// Localized label for a runtime-reported apply impact.
fn impact_label(impact: apply::SettingsImpact) -> String {
    if impact.app_restart {
        i18n_embed_fl::fl!(crate::i18n::loader(), "impact-app-restart")
    } else if impact.plugin_restart {
        i18n_embed_fl::fl!(crate::i18n::loader(), "impact-plugin-restart")
    } else if impact.runtime_reload {
        i18n_embed_fl::fl!(crate::i18n::loader(), "impact-runtime-reload")
    } else {
        i18n_embed_fl::fl!(crate::i18n::loader(), "impact-immediate")
    }
}

fn memory_mode_for_section(section: &str) -> Option<crate::settings::MemoryPageMode> {
    match section {
        "memory-browse" => Some(crate::settings::MemoryPageMode::Browse),
        "memory-recall" => Some(crate::settings::MemoryPageMode::RecallSearch),
        "memory-pending" => Some(crate::settings::MemoryPageMode::PendingApproval),
        "memory-commitments" => Some(crate::settings::MemoryPageMode::Commitments),
        _ => None,
    }
}

fn prepare_section_focus(world: &mut World, ui_entity: Entity, section: &str) {
    if let Some(mode) = memory_mode_for_section(section)
        && let Some(mut state) = world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
    {
        state.0.memory_journal_mode = mode;
    }
}

#[derive(Debug)]
pub struct SettingsUi {
    pub current_page: PageKind,
    pub input: SettingsInputState,
    /// Draft holding every pending settings edit; pages write here and the
    /// apply bar pushes it through validation → persist → runtime apply.
    pub draft: draft::SettingsDraft,
    /// Shown as a banner until dismissed.
    pub apply_feedback: Option<ApplyFeedback>,
    /// In-flight async apply preparation (validation + secret merge).
    apply_prepare: input::AsyncData<apply::ApplyPrepare>,
    /// In-flight async runtime apply receiver.
    apply_rx:
        Option<tokio::sync::oneshot::Receiver<Result<std::collections::BTreeSet<String>, String>>>,
    /// Real config captured before persist, used for rollback.
    apply_original: Option<ene_config::EneConfig>,
    /// Whether an apply (preparation or finalize) is currently running.
    applying: bool,
    pub animation: AnimationControl,
    pub emotion_queue: EmotionQueue,
    /// When the runtime was constructed. Used for `now_secs` in
    /// emotion-queue timestamps.
    pub started_at: Instant,
    /// Settings search box text; results are recomputed when it or the locale changes.
    pub search_query: String,
    search_language: Language,
    search_computed_for: String,
    search_hits: Vec<SearchHit>,
    /// One-shot Advanced-page filter applied from a schema-leaf search hit.
    advanced_filter: Option<String>,
    /// One-shot plugin-card focus applied from a plugin/action search hit.
    plugin_focus: Option<String>,
    /// Consumed on the next frame to switch page and highlight a section.
    pending_focus: Option<(PageKind, Option<&'static str>)>,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self {
            current_page: PageKind::Overview,
            input: SettingsInputState::new(),
            draft: draft::SettingsDraft::new(ene_config::EneConfig::default()),
            apply_feedback: None,
            apply_prepare: input::AsyncData::new(),
            apply_rx: None,
            apply_original: None,
            applying: false,
            animation: AnimationControl::new(),
            emotion_queue: EmotionQueue::default(),
            started_at: Instant::now(),
            search_query: String::new(),
            search_language: Language::default(),
            search_computed_for: String::new(),
            search_hits: Vec::new(),
            advanced_filter: None,
            plugin_focus: None,
            pending_focus: None,
        }
    }

    /// Mirror the on-disk `CharacterSettings` into the editable text
    /// buffers. The runtime calls this when the settings window
    /// transitions from hidden → visible.
    pub fn sync_from_settings(
        &mut self,
        settings: &CharacterSettings,
        ui_state: &crate::settings::UiState,
    ) {
        self.input.sync_from_settings(settings, ui_state);
        self.draft.resync(settings.config());
        if let Some(page) = ui_state.focused_page {
            self.current_page = page;
        }
    }

    /// Runs the apply pipeline for the pending draft.
    ///
    /// Called from the window-level Apply button. On success the persisted
    /// config is refreshed into the draft; the feedback banner shows the
    /// reported impact (immediate / hot-reload / plugin restart / app
    /// restart) or the validation/runtime errors.
    pub fn apply_pending(&mut self, settings: &CharacterSettings, ai: &Arc<CoreSession>) {
        if self.applying || self.apply_prepare.loading() {
            return;
        }
        self.draft.validate();
        if self.draft.has_issues() {
            self.apply_feedback = Some(ApplyFeedback {
                revision: self.draft.revision(),
                ok: false,
                impact: apply::SettingsImpact::default(),
                applied_sections: Vec::new(),
                message: Some(
                    self.draft
                        .all_issues()
                        .into_iter()
                        .map(|issue| format!("{}: {}", issue.path, issue.message))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            });
            return;
        }
        let original = settings.config();
        let editing = self.draft.editing().clone();
        let dirty = self.draft.dirty_paths().clone();
        self.applying = true;
        self.apply_prepare = input::AsyncData::new();
        self.apply_prepare.start(
            ai.spawn_fetch(async move { apply::prepare_apply(&original, &editing, &dirty) }),
        );
    }

    /// Pumps the async apply preparation; a clean result is finalized
    /// (persist + runtime apply), errors are surfaced without touching disk.
    fn pump_apply(&mut self, settings: &CharacterSettings, ai: &Arc<CoreSession>) {
        if !self.applying {
            return;
        }
        self.apply_prepare.poll();
        if let Some(prepare) = self.apply_prepare.data.take() {
            self.apply_prepare = input::AsyncData::new();
            if !prepare.errors.is_empty() {
                self.applying = false;
                self.apply_feedback = Some(ApplyFeedback {
                    revision: self.draft.revision(),
                    ok: false,
                    impact: apply::SettingsImpact::default(),
                    applied_sections: Vec::new(),
                    message: Some(prepare.errors.join("\n")),
                });
                return;
            }
            let original = settings.config();
            self.apply_original = Some(original.clone());
            match apply::begin_finalize(settings, &self.draft, ai.as_ref(), prepare.proposed) {
                Ok(receiver) => {
                    self.apply_rx = Some(receiver);
                }
                Err(error) => {
                    self.applying = false;
                    self.apply_feedback = Some(ApplyFeedback {
                        revision: self.draft.revision(),
                        ok: false,
                        impact: apply::SettingsImpact::default(),
                        applied_sections: Vec::new(),
                        message: Some(error.to_string()),
                    });
                }
            }
            return;
        }
        if let Some(receiver) = &mut self.apply_rx
            && let Ok(result) = receiver.try_recv()
        {
            self.apply_rx = None;
            self.applying = false;
            let original = self.apply_original.take().unwrap_or_default();
            match apply::finish_finalize(settings, &mut self.draft, result, original) {
                Ok(outcome) => self.handle_apply_outcome(settings, ai, outcome),
                Err(error) => {
                    self.apply_feedback = Some(ApplyFeedback {
                        revision: self.draft.revision(),
                        ok: false,
                        impact: apply::SettingsImpact::default(),
                        applied_sections: Vec::new(),
                        message: Some(error.to_string()),
                    });
                }
            }
        }
    }

    fn handle_apply_outcome(
        &mut self,
        settings: &CharacterSettings,
        ai: &Arc<CoreSession>,
        outcome: apply::ApplyOutcome,
    ) {
        // A committed secret never stays in a UI text buffer, even when it
        // was typed this session.
        self.input.ai_api_key.clear();
        if outcome.conflicted {
            // Keep the user's edits; only the baseline moves to the actor's
            // newer state so the next apply is not stale.
            self.draft.resync_baseline(settings.config());
            self.apply_feedback = Some(ApplyFeedback {
                revision: outcome.revision,
                ok: false,
                impact: apply::SettingsImpact::default(),
                applied_sections: Vec::new(),
                message: Some(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "settings-apply-conflict"
                )),
            });
        } else {
            let previous = self.draft.editing().clone();
            self.draft.resync(settings.config());
            self.draft.keep_core_extras(&previous);
            self.input.ai_pickers_seeded = false;
            self.input.core_settings.restart(ai.fetch_core_settings());
            self.apply_feedback = Some(ApplyFeedback {
                revision: outcome.revision,
                ok: outcome.ok(),
                impact: outcome.impact,
                applied_sections: outcome.applied_sections.clone(),
                message: None,
            });
        }
    }

    pub fn discard_pending(&mut self, settings: &CharacterSettings) {
        self.draft.resync(settings.config());
        self.input.ai_pickers_seeded = false;
        self.apply_feedback = None;
    }

    /// Whether a non-empty search is active; pages use this to auto-reveal
    /// hidden (`x-ene-ui.advanced`) fields.
    #[must_use]
    pub fn search_active(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    /// Render the full settings window. The caller is expected to
    /// have already opened an egui pass and supplied a `Ui` (via
    /// `egui::CentralPanel::show_inside`).
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut CharacterSettings,
        ai: Option<&Arc<CoreSession>>,
        world: &mut World,
        ui_entity: Entity,
        now_secs: f64,
    ) {
        self.sync_search_language(settings.language());
        crate::theme::apply_egui_visuals(ui.ctx());

        let mut dismiss_fatal = false;
        let fatal_message = world
            .get::<crate::component::ui::UiStateComponent>(ui_entity)
            .and_then(|state| state.0.runtime_startup_error.clone())
            .filter(|_| {
                world
                    .get::<crate::component::ui::UiStateComponent>(ui_entity)
                    .is_some_and(|state| !state.0.fatal_startup_dismissed)
            });
        if let Some(message) = fatal_message {
            egui::Window::new(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "runtime-fatal-title"
            ))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "runtime-fatal-body",
                    message = message
                ));
                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "runtime-fatal-dismiss"
                    ))
                    .clicked()
                {
                    dismiss_fatal = true;
                }
            });
        }
        if dismiss_fatal
            && let Some(mut state) =
                world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
        {
            state.0.fatal_startup_dismissed = true;
        }

        let mut reconnect_requested = false;
        let show_disconnect_banner = world
            .get::<crate::component::ui::UiStateComponent>(ui_entity)
            .is_some_and(|state| state.0.runtime_disconnected);
        if show_disconnect_banner {
            let reconnect_attempted = world
                .get::<crate::component::ui::UiStateComponent>(ui_entity)
                .is_some_and(|state| state.0.reconnect_attempted);
            egui::Window::new(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "runtime-disconnected-title"
            ))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
            .show(ui.ctx(), |ui| {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-disconnected-body"),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !reconnect_attempted,
                            egui::Button::new(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "runtime-reconnect"
                            )),
                        )
                        .clicked()
                    {
                        reconnect_requested = true;
                    }
                });
                if reconnect_attempted {
                    ui.weak(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "runtime-reconnect-already-attempted"
                    ));
                }
            });
        }
        if reconnect_requested
            && let Some(mut state) =
                world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
        {
            state.0.reconnect_requested = true;
        }

        let Some(ai) = ai else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-unavailable"),
            );
            return;
        };

        let mut open_ai = false;
        let mut dismiss = false;
        let show_onboarding = world
            .get::<crate::component::ui::UiStateComponent>(ui_entity)
            .is_some_and(|s| s.0.show_onboarding);
        if show_onboarding {
            egui::Window::new(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "onboarding-title"
            ))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 24.0])
            .show(ui.ctx(), |ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "onboarding-body"));
                ui.horizontal(|ui| {
                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "onboarding-open-settings"
                        ))
                        .clicked()
                    {
                        open_ai = true;
                    }
                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "onboarding-dismiss"
                        ))
                        .clicked()
                    {
                        dismiss = true;
                    }
                });
            });
        }
        if (open_ai || dismiss)
            && let Some(mut state) =
                world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
        {
            if open_ai {
                state.0.focused_page = Some(PageKind::Ai);
                state.0.settings_window_visible = true;
                state.0.show_onboarding = false;
            }
            if dismiss {
                state.0.show_onboarding = false;
            }
        }
        if open_ai {
            self.current_page = PageKind::Ai;
        }

        // Advance the async apply pipeline even while the bar is hidden so
        // a finishing apply updates the draft and feedback banner.
        self.pump_apply(settings, ai);

        if let Some(mut state) = world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
            && let Some(page) = state.0.focused_page.take()
        {
            self.current_page = page;
        }

        // Consume a pending search navigation before the page renders so
        // the section focus request is visible to this frame's cards.
        if let Some((page, section)) = self.pending_focus.take() {
            self.current_page = page;
            if let Some(section) = section {
                prepare_section_focus(world, ui_entity, section);
                components::request_section_focus(ui.ctx(), section);
            }
        }
        if let Some(filter) = self.advanced_filter.take() {
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("advanced_filter"), filter);
            });
        }
        // The bar must be laid out before the content panels: the nested
        // side/central panels consume every remaining point, so rendering it
        // afterwards would place it below the visible area.
        if self.apply_feedback.is_some() || self.draft.is_dirty() {
            egui::Panel::bottom("settings_apply_bar")
                .resizable(false)
                .show_separator_line(false)
                .show(ui, |ui| {
                    self.render_apply_bar(ui, settings, Some(ai));
                });
        }
        let mut selected_page: Option<PageKind> = None;
        let narrow = use_compact_navigation(ui.available_width());
        // An explicit id anchors the page's auto-id chain so the apply bar
        // appearing or disappearing cannot shift every widget id (and with
        // it widget state). Nested panels take their ids from the parent ui
        // salt, so without this scope the sidebar/page ids depend on how
        // many widgets the bar rendered in the same frame.
        ui.scope_builder(
            egui::UiBuilder::new().id(egui::Id::new("settings_content")),
            |ui| {
                if narrow {
                    self.render_compact_nav(ui, &mut selected_page);
                    ui.separator();
                    self.render_page_content(
                        ui,
                        settings,
                        ai,
                        world,
                        ui_entity,
                        now_secs,
                        &mut selected_page,
                    );
                } else {
                    egui::Panel::left("settings_nav_sidebar")
                        .default_size(212.0)
                        .min_size(180.0)
                        .resizable(false)
                        .show(ui, |ui| {
                            self.render_sidebar(ui, &mut selected_page);
                        });
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 8)))
                        .show(ui, |ui| {
                            self.render_page_content(
                                ui,
                                settings,
                                ai,
                                world,
                                ui_entity,
                                now_secs,
                                &mut selected_page,
                            );
                        });
                }
            },
        );
        if let Some(page) = selected_page {
            self.current_page = page;
        }

        // Rendered last so it floats above every page: a close, app exit,
        // reload, or character switch with unsaved card edits must confirm
        // before the action proceeds.
        page_character_editor::render_discard_dialog(
            ui,
            settings,
            &mut self.emotion_queue,
            now_secs,
            world,
            ui_entity,
        );
        self.render_settings_close_dialog(ui, settings, world, ui_entity);
    }

    /// Confirms a window close while the draft holds pending edits: the
    /// runtime defers the hide and sets `settings_close_requested`, and this
    /// modal lets the user discard (which clears the draft and lets the
    /// deferred close complete) or keep editing.
    fn render_settings_close_dialog(
        &mut self,
        ui: &mut egui::Ui,
        settings: &CharacterSettings,
        world: &mut World,
        ui_entity: Entity,
    ) {
        let pending = world
            .get::<crate::component::ui::UiStateComponent>(ui_entity)
            .is_some_and(|state| state.0.settings_close_requested);
        if !pending {
            return;
        }
        let mut discard = false;
        let mut keep_editing = false;
        egui::Modal::new(egui::Id::new("settings-close-discard-modal")).show(ui.ctx(), |ui| {
            ui.set_min_width(280.0);
            ui.heading(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "settings-discard-title"
            ));
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "settings-discard-body"
            ));
            ui.horizontal(|ui| {
                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "settings-discard-close"
                    ))
                    .clicked()
                {
                    discard = true;
                }
                if ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "settings-keep-editing"
                    ))
                    .clicked()
                {
                    keep_editing = true;
                }
            });
        });
        if discard {
            // Discarding clears the draft; the runtime's per-frame close
            // check then completes the deferred hide.
            self.discard_pending(settings);
            if let Some(mut state) =
                world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
            {
                state.0.settings_close_requested = true;
            }
        } else if keep_editing
            && let Some(mut state) =
                world.get_mut::<crate::component::ui::UiStateComponent>(ui_entity)
        {
            state.0.settings_close_requested = false;
        }
    }

    fn render_apply_bar(
        &mut self,
        ui: &mut egui::Ui,
        settings: &CharacterSettings,
        ai: Option<&Arc<CoreSession>>,
    ) {
        let mut dismiss_feedback = false;
        if let Some(feedback) = &self.apply_feedback {
            let (color, message) = if feedback.ok {
                let impact = impact_label(feedback.impact);
                (
                    egui::Color32::from_rgb(0x2e, 0x7d, 0x32),
                    i18n_embed_fl::fl!(crate::i18n::loader(), "settings-apply-ok", impact = impact),
                )
            } else {
                (
                    egui::Color32::from_rgb(0xc6, 0x28, 0x28),
                    i18n_embed_fl::fl!(crate::i18n::loader(), "settings-apply-failed"),
                )
            };
            egui::Frame::new()
                .fill(color.gamma_multiply(0.18))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(color, message);
                        if let Some(detail) = &feedback.message {
                            ui.weak(detail);
                        } else if !feedback.applied_sections.is_empty() {
                            ui.weak(format!(
                                "{}: {} (rev {})",
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "settings-applied-sections"
                                ),
                                feedback.applied_sections.join(", "),
                                feedback.revision
                            ));
                        } else {
                            ui.weak(format!("rev {}", feedback.revision));
                        }
                        if ui.small_button("✕").clicked() {
                            dismiss_feedback = true;
                        }
                    });
                });
            ui.add_space(4.0);
        }
        if dismiss_feedback {
            self.apply_feedback = None;
        }

        if !self.draft.is_dirty() {
            return;
        }
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(0x33, 0x35, 0x3b))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.applying {
                        ui.weak(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "settings-apply-validating"
                        ));
                    }
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "settings-draft-pending",
                        revision = self.draft.revision(),
                        count = self.draft.dirty_paths().len(),
                        applied = self.draft.applied_revision()
                    ));
                    if ui
                        .add_enabled(
                            !self.draft.has_issues() && !self.applying,
                            egui::Button::new(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "settings-apply"
                            )),
                        )
                        .on_hover_text(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "settings-apply-hint"
                        ))
                        .clicked()
                        && let Some(ai) = ai
                    {
                        self.apply_pending(settings, ai);
                    }
                    if ui
                        .button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "settings-discard"
                        ))
                        .clicked()
                    {
                        self.discard_pending(settings);
                    }
                });
                if self.draft.has_issues() {
                    for issue in self.draft.all_issues() {
                        ui.colored_label(
                            egui::Color32::from_rgb(0xff, 0x8a, 0x65),
                            format!("{}: {}", issue.path, issue.message),
                        );
                    }
                }
            });
    }

    fn recompute_search(&mut self) {
        if self.search_query == self.search_computed_for {
            return;
        }
        self.search_computed_for = self.search_query.clone();
        let plugin_names: Vec<String> = self
            .input
            .plugins
            .data
            .as_ref()
            .map(|plugins| plugins.iter().map(|plugin| plugin.plugin.clone()).collect())
            .unwrap_or_default();
        self.search_hits = compute_search(&self.search_query, &plugin_names);
    }

    fn sync_search_language(&mut self, language: Language) {
        if self.search_language == language {
            return;
        }
        self.search_language = language;
        self.search_computed_for.clear();
        self.search_hits.clear();
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui, selected_page: &mut Option<PageKind>) {
        ui.add_space(8.0);
        self.render_search_input(ui);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("settings_nav_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if self.search_query.trim().is_empty() {
                    for category in PageCategory::ALL {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(category.label())
                                .small()
                                .weak()
                                .strong(),
                        );
                        for page in PageKind::ALL {
                            if page.meta().category != category {
                                continue;
                            }
                            let selected = self.current_page == page;
                            if ui.selectable_label(selected, page.title()).clicked() {
                                *selected_page = Some(page);
                            }
                        }
                    }
                } else {
                    self.render_search_results(ui);
                }
            });
    }

    fn render_compact_nav(&mut self, ui: &mut egui::Ui, selected_page: &mut Option<PageKind>) {
        egui::ComboBox::from_id_salt("settings_compact_page_picker")
            .selected_text(self.current_page.title())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for category in PageCategory::ALL {
                    ui.label(
                        egui::RichText::new(category.label())
                            .small()
                            .weak()
                            .strong(),
                    );
                    for page in PageKind::ALL {
                        if page.meta().category != category {
                            continue;
                        }
                        let selected = self.current_page == page;
                        if ui.selectable_label(selected, page.title()).clicked() {
                            *selected_page = Some(page);
                        }
                    }
                    ui.separator();
                }
            });
        ui.add_space(4.0);
        self.render_search_input(ui);
        if !self.search_query.trim().is_empty() {
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("settings_compact_search_results")
                .max_height(180.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| self.render_search_results(ui));
        }
    }

    fn render_page_content(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut CharacterSettings,
        ai: &Arc<CoreSession>,
        world: &mut World,
        ui_entity: Entity,
        now_secs: f64,
        _selected_page: &mut Option<PageKind>,
    ) {
        let title = self.current_page.title();
        let description = self.current_page.description();
        components::page_header(ui, &title, &description);
        ui.separator();
        let reveal_advanced = self.search_active();
        let plugin_focus = self.plugin_focus.take();
        egui::ScrollArea::vertical()
            .id_salt("settings_page_scroll")
            .hscroll(false)
            .auto_shrink([false; 2])
            .show_viewport(ui, |ui, viewport| {
                if self.current_page == PageKind::Voice {
                    ui.set_max_width(viewport.width());
                    ui.set_width(viewport.width());
                }
                match self.current_page {
                    PageKind::Overview => page_overview::render(
                        ui,
                        settings,
                        ai,
                        &mut self.input,
                        &mut self.current_page,
                        self.apply_feedback.as_ref(),
                    ),
                    PageKind::Character => page_character::render(
                        ui,
                        settings,
                        &mut self.animation,
                        ai,
                        &mut self.input,
                        &mut self.emotion_queue,
                        now_secs,
                        world,
                        ui_entity,
                    ),
                    PageKind::CharacterEditor => {
                        page_character_editor::render(ui, settings, ai, world, ui_entity);
                    }
                    PageKind::General => {
                        page_graphics::render(
                            ui,
                            settings,
                            &mut self.draft,
                            &mut self.animation,
                            ai,
                            world,
                            ui_entity,
                        );
                        page_accessibility::render(ui, settings);
                    }
                    PageKind::Ai => page_ai::render(
                        ui,
                        settings,
                        &mut self.draft,
                        &mut self.animation,
                        ai,
                        &mut self.input,
                        world,
                        ui_entity,
                    ),
                    PageKind::Voice => {
                        page_voice::render(
                            ui,
                            settings,
                            &mut self.draft,
                            ai,
                            &mut self.input,
                            world,
                        );
                    }
                    PageKind::Features => {
                        page_features::render(ui, &mut self.draft, ai, &mut self.input);
                    }
                    PageKind::Memory => page_memory::render_config(
                        ui,
                        settings,
                        &mut self.draft,
                        ai,
                        &mut self.input,
                    ),
                    PageKind::Memories => {
                        page_memory::render_journal(ui, ai, &mut self.input, world, ui_entity);
                        page_memory_ledger::render(ui, ai, &mut self.input, world, ui_entity);
                    }
                    PageKind::Permissions => {
                        page_permissions::render(
                            ui,
                            settings,
                            ai,
                            &mut self.input,
                            world,
                            ui_entity,
                        );
                    }
                    PageKind::Approvals => {
                        page_approvals::render(
                            ui,
                            settings,
                            &mut self.draft,
                            ai,
                            &mut self.input,
                            world,
                            ui_entity,
                        );
                    }
                    PageKind::Connectors => {
                        page_connectors::render(ui, ai, &mut self.input, world, ui_entity);
                    }
                    PageKind::Sessions => {
                        page_sessions::render(ui, ai, &mut self.input, world, ui_entity);
                    }
                    PageKind::Schedules => page_schedules::render(ui, ai, &mut self.input),
                    PageKind::Plugins => {
                        page_plugins::render(
                            ui,
                            settings,
                            &mut self.draft,
                            ai,
                            &mut self.input,
                            world,
                            ui_entity,
                            plugin_focus.as_deref(),
                        );
                    }
                    PageKind::Engines => {
                        page_engines::render(ui, &mut self.draft, ai, &mut self.input);
                    }
                    PageKind::Advanced => {
                        page_advanced::render(ui, &mut self.draft, reveal_advanced);
                    }
                    PageKind::Diagnostics => {
                        page_debug::render(ui, settings, &mut self.animation, ai, world, ui_entity);
                    }
                }
            });
    }

    fn render_search_input(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let clear_width = if self.search_query.is_empty() {
                0.0
            } else {
                52.0
            };
            let input_width =
                (ui.available_width() - clear_width - ui.spacing().item_spacing.x).max(80.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "settings-search-placeholder"
                    ))
                    .desired_width(input_width),
            );
            if !self.search_query.is_empty()
                && ui
                    .button(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "settings-search-clear"
                    ))
                    .clicked()
            {
                self.search_query.clear();
            }
        });
        self.recompute_search();
    }

    fn render_search_results(&mut self, ui: &mut egui::Ui) {
        if self.search_query.trim().is_empty() {
            return;
        }
        if self.search_hits.is_empty() {
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "settings-search-empty",
                query = self.search_query.trim()
            ));
            return;
        }
        let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let mut chosen: Option<usize> = None;
        for (index, hit) in self.search_hits.iter().enumerate() {
            let label = match hit.section {
                Some(_) => format!("{} · {}", hit.title, hit.detail),
                None => format!("{} — {}", hit.title, hit.detail),
            };
            if ui.selectable_label(false, label).clicked() {
                chosen = Some(index);
            }
        }
        if let Some(index) = chosen.or_else(|| enter_pressed.then_some(0))
            && let Some(hit) = self.search_hits.get(index).cloned()
        {
            self.pending_focus = Some((hit.page, hit.section));
            self.advanced_filter = hit.filter;
            self.plugin_focus = hit.plugin_focus;
        }
    }
}

impl Default for SettingsUi {
    fn default() -> Self {
        Self::new()
    }
}

fn noto_sans_jp_font_definitions() -> Option<&'static egui::FontDefinitions> {
    static FONTS: OnceLock<Option<egui::FontDefinitions>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let assets_dir = ene_config::paths::assets_dir();
            let font_path = assets_dir.join("fonts").join("NotoSansJP-Regular.ttf");
            if !font_path.exists() {
                tracing::warn!("Font file does not exist at {:?}", font_path);
                return None;
            }
            let font_data = match std::fs::read(&font_path) {
                Ok(data) => data,
                Err(error) => {
                    tracing::warn!(%error, path = ?font_path, "Failed to read font file");
                    return None;
                }
            };

            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "NotoSansJP".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                prop.insert(0, "NotoSansJP".to_owned());
            }
            if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                mono.insert(0, "NotoSansJP".to_owned());
            }
            tracing::info!("Successfully loaded NotoSansJP-Regular.ttf for egui");
            Some(fonts)
        })
        .as_ref()
}

/// Install `NotoSansJP` into a dedicated egui context. Each winit window
/// owns its own [`egui::Context`], so fonts must be applied per context
/// (not once process-wide).
pub fn apply_egui_fonts(ctx: &egui::Context) {
    if let Some(fonts) = noto_sans_jp_font_definitions() {
        ctx.set_fonts(fonts.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_is_unique_and_has_one_category() {
        assert_eq!(PageKind::ALL.len(), 18);
        for (index, page) in PageKind::ALL.iter().enumerate() {
            assert!(PageKind::ALL[index + 1..].iter().all(|other| other != page));
        }
        let counts = PageCategory::ALL.map(|category| {
            PageKind::ALL
                .iter()
                .filter(|page| page.meta().category == category)
                .count()
        });
        assert_eq!(counts, [10, 8]);
        assert_eq!(counts.into_iter().sum::<usize>(), PageKind::ALL.len());
    }

    #[test]
    fn every_page_title_description_and_section_resolves() {
        let mut metadata_sections = Vec::new();
        for page in PageKind::ALL {
            let meta = page.meta();
            let title = page.title();
            let description = page.description();
            assert!(!title.is_empty());
            assert_ne!(title, meta.title_key);
            assert!(!description.is_empty());
            assert_ne!(description, meta.description_key);
            assert!(!meta.sections.is_empty());
            for section in meta.sections {
                assert!(!metadata_sections.contains(section));
                metadata_sections.push(*section);
                let title = section_title(section);
                assert!(title.as_ref().is_some_and(|value| !value.is_empty()));
            }
            for alias in meta.section_aliases {
                assert!(meta.sections.contains(&alias.section));
                assert!(!alias.alias.is_empty());
            }
        }
        assert_eq!(
            metadata_sections.len(),
            components::SECTION_TITLE_KEYS.len()
        );
        for (section, _) in components::SECTION_TITLE_KEYS {
            assert!(metadata_sections.contains(&section));
        }
    }

    #[test]
    fn technical_voice_aliases_target_their_sections() {
        for (query, expected) in [
            ("tts", "voice-tts"),
            ("stt", "voice-stt"),
            ("vad", "voice-mic"),
            ("mic", "voice-mic"),
        ] {
            let hits = compute_search(query, &[]);
            assert_eq!(hits.first().and_then(|hit| hit.section), Some(expected));
            assert_eq!(hits.first().map(|hit| hit.page), Some(PageKind::Voice));
        }
    }

    #[test]
    fn search_ranks_page_titles_before_other_page_matches() {
        let hits = compute_search(&PageKind::Voice.title(), &[]);
        let first = hits.first().unwrap();
        assert_eq!(first.page, PageKind::Voice);
        assert_eq!(first.section, None);
        assert_eq!(first.rank, 0);
    }

    #[test]
    fn search_ranks_section_aliases_before_page_aliases() {
        let hits = compute_search("tts", &[]);
        let section = hits
            .iter()
            .position(|hit| hit.section == Some("voice-tts"))
            .unwrap();
        let page = hits
            .iter()
            .position(|hit| hit.page == PageKind::Voice && hit.section.is_none())
            .unwrap();
        assert!(section < page);
        assert_eq!(hits[section].rank, 1);
        assert_eq!(hits[page].rank, 2);
    }

    #[test]
    fn empty_search_has_no_results() {
        assert!(compute_search("", &[]).is_empty());
        assert!(compute_search("   ", &[]).is_empty());
    }

    #[test]
    fn search_cache_invalidates_when_language_changes() {
        let mut ui = SettingsUi::new();
        ui.search_query = "voice".to_string();
        ui.search_computed_for = ui.search_query.clone();
        ui.search_hits.push(SearchHit {
            page: PageKind::Voice,
            section: None,
            title: "Voice".to_string(),
            detail: "Voice settings".to_string(),
            filter: None,
            plugin_focus: None,
            rank: 0,
        });

        ui.sync_search_language(Language::Ja);

        assert_eq!(ui.search_language, Language::Ja);
        assert!(ui.search_computed_for.is_empty());
        assert!(ui.search_hits.is_empty());
    }

    #[test]
    fn compact_navigation_boundary_is_exclusive() {
        assert!(use_compact_navigation(719.0));
        assert!(!use_compact_navigation(720.0));
    }

    #[test]
    fn apply_bar_lays_out_above_the_bottom_edge_and_never_overlaps_content() {
        // The apply bar must be laid out before the nested side/central
        // panels: those panels consume every remaining point, so a bar
        // rendered after them lands below the visible area.
        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 640.0),
            )),
            ..Default::default()
        };
        let mut bar_rect = egui::Rect::NOTHING;
        let mut content_rect = egui::Rect::NOTHING;
        // Two frames: the bottom panel sizes itself to its content from the
        // previous frame, so the steady-state layout only exists on frame 2.
        for _ in 0..2 {
            let _output = context.run_ui(input.clone(), |ui| {
                egui::Panel::bottom("settings_apply_bar")
                    .resizable(false)
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        let response = egui::Frame::new()
                            .fill(egui::Color32::from_rgb(0x33, 0x35, 0x3b))
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("pending");
                                    ui.button("Apply").clicked();
                                });
                            })
                            .response;
                        bar_rect = response.rect;
                    });
                egui::Panel::left("settings_nav_sidebar")
                    .default_size(212.0)
                    .min_size(180.0)
                    .resizable(false)
                    .show(ui, |ui| {
                        ui.label("nav");
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 8)))
                    .show(ui, |ui| {
                        let response = egui::ScrollArea::vertical()
                            .id_salt("settings_page_scroll")
                            .hscroll(false)
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                ui.label("content");
                            });
                        content_rect = response.inner_rect;
                    });
            });
        }
        let visible = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 640.0));
        assert!(
            visible.contains_rect(bar_rect),
            "apply bar {bar_rect:?} must stay inside the visible area {visible:?}"
        );
        assert!(
            bar_rect.top() >= content_rect.bottom(),
            "apply bar {bar_rect:?} overlaps content {content_rect:?}"
        );
        assert!(
            bar_rect.bottom() <= visible.bottom() + 0.5,
            "apply bar {bar_rect:?} must sit at the bottom of the window"
        );
    }

    #[test]
    fn apply_bar_toggle_does_not_repaint_page_widget_ids() {
        // egui debug builds paint a red 2px outline around every widget
        // whose id changes rect between passes (`warn_if_rect_changes_id`).
        // The page content must keep its auto-id chain anchored at a stable
        // id so showing or hiding the apply bar cannot shift every page
        // widget id (and with it widget state) for one frame.
        fn has_red_stroke(shape: &egui::Shape) -> bool {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().any(has_red_stroke),
                egui::Shape::Rect(rect) => {
                    let [r, g, b, a] = rect.stroke.color.to_array();
                    rect.stroke.width > 0.0 && a > 0 && r > 120 && g < 110 && b < 110
                }
                _ => false,
            }
        }

        let context = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        let settings = crate::settings::CharacterSettings {
            assets_dir: std::path::PathBuf::from("/tmp"),
            characters: Vec::new(),
            character_state: crate::settings::CharacterState::default(),
            store: std::sync::Arc::new(parking_lot::RwLock::new(
                ene_config::ConfigStore::from_config(ene_config::EneConfig::default()),
            )),
            character_store: ene_card::CharacterConfigStore::default(),
        };
        let mut ui_state = SettingsUi::new();
        for frame in 0..6 {
            if frame == 2 {
                ui_state
                    .draft
                    .set_path("mind.language", serde_json::Value::String("ja".into()));
            }
            if frame == 5 {
                ui_state.draft.resync(ene_config::EneConfig::default());
            }
            let dirty = ui_state.draft.is_dirty();
            let output = context.run_ui(input.clone(), |ui| {
                if dirty {
                    egui::Panel::bottom("settings_apply_bar")
                        .resizable(false)
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            ui_state.render_apply_bar(ui, &settings, None);
                        });
                }
                ui.scope_builder(
                    egui::UiBuilder::new().id(egui::Id::new("settings_content")),
                    |ui| {
                        egui::Panel::left("settings_nav_sidebar")
                            .default_size(212.0)
                            .min_size(180.0)
                            .resizable(false)
                            .show(ui, |ui| {
                                ui.label("nav");
                            });
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 8)))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("settings_page_scroll")
                                    .hscroll(false)
                                    .auto_shrink([false; 2])
                                    .show(ui, |ui| {
                                        ui.label("content");
                                    });
                            });
                    },
                );
            });
            for clipped in &output.shapes {
                assert!(
                    !has_red_stroke(&clipped.shape),
                    "red id-warning rect on frame {frame} while the apply bar is {}",
                    if dirty { "shown" } else { "hidden" }
                );
            }
        }
    }

    #[test]
    fn memory_sections_select_the_matching_mode() {
        use crate::settings::MemoryPageMode;

        assert_eq!(
            memory_mode_for_section("memory-browse"),
            Some(MemoryPageMode::Browse)
        );
        assert_eq!(
            memory_mode_for_section("memory-recall"),
            Some(MemoryPageMode::RecallSearch)
        );
        assert_eq!(
            memory_mode_for_section("memory-pending"),
            Some(MemoryPageMode::PendingApproval)
        );
        assert_eq!(
            memory_mode_for_section("memory-commitments"),
            Some(MemoryPageMode::Commitments)
        );
        assert_eq!(memory_mode_for_section("voice-tts"), None);
    }
}
