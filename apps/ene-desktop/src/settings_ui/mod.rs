//! Settings UI — the tabbed settings window.
//!
//! The runtime owns a single [`SettingsUi`] per `UiWindow`. Each
//! frame the `UiWindow` calls [`SettingsUi::render`] with the live
//! `&mut CharacterSettings` and the `Arc<AiBridge>`.
pub mod components;
pub mod input;
pub mod page_accessibility;
pub mod page_ai;
pub mod page_approvals;
pub mod page_character;
pub mod page_character_editor;
pub mod page_connectors;
pub mod page_debug;
pub mod page_features;
pub mod page_graphics;
pub mod page_memory;
pub mod page_memory_ledger;
pub mod page_permissions;
pub mod page_sessions;
pub mod page_voice;
pub mod widgets;

pub use components::section_title;
pub use input::SettingsInputState;

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionQueue};
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use components::NARROW_NAV_THRESHOLD;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    #[default]
    Character,
    /// Character Card (`CCv3`) editor.
    CharacterEditor,
    Graphics,
    Ai,
    Voice,
    Features,
    Accessibility,
    Memory,
    MemoryLedger,
    Permissions,
    Approvals,
    Connectors,
    Sessions,
    Debug,
}

/// Navigation grouping shown in the settings sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCategory {
    Basics,
    AiVoice,
    DataAccess,
    System,
}

impl PageCategory {
    pub const ALL: [PageCategory; 4] = [
        PageCategory::Basics,
        PageCategory::AiVoice,
        PageCategory::DataAccess,
        PageCategory::System,
    ];

    pub fn label(self) -> String {
        let key = match self {
            PageCategory::Basics => "page-category-basics",
            PageCategory::AiVoice => "page-category-ai-voice",
            PageCategory::DataAccess => "page-category-data-access",
            PageCategory::System => "page-category-system",
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
    pub const ALL: [PageKind; 14] = [
        PageKind::Character,
        PageKind::CharacterEditor,
        PageKind::Graphics,
        PageKind::Ai,
        PageKind::Voice,
        PageKind::Features,
        PageKind::Accessibility,
        PageKind::Memory,
        PageKind::MemoryLedger,
        PageKind::Sessions,
        PageKind::Permissions,
        PageKind::Approvals,
        PageKind::Connectors,
        PageKind::Debug,
    ];

    pub fn meta(self) -> &'static PageMeta {
        match self {
            PageKind::Character => &PageMeta {
                category: PageCategory::Basics,
                title_key: "character",
                description_key: "page-character-description",
                aliases: &["vrm", "motion", "expression", "animation"],
                sections: &[
                    "character-model",
                    "character-transform",
                    "character-expressions",
                ],
                section_aliases: &[],
            },
            PageKind::CharacterEditor => &PageMeta {
                category: PageCategory::Basics,
                title_key: "character-card",
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
            PageKind::Graphics => &PageMeta {
                category: PageCategory::Basics,
                title_key: "display",
                description_key: "page-graphics-description",
                aliases: &[
                    "graphics", "display", "quality", "language", "theme", "dark", "light",
                ],
                sections: &["graphics-quality", "graphics-language", "graphics-theme"],
                section_aliases: &[],
            },
            PageKind::Ai => &PageMeta {
                category: PageCategory::AiVoice,
                title_key: "ai",
                description_key: "page-ai-description",
                aliases: &["openai", "api", "chat", "embedding", "provider", "model"],
                sections: &["ai-chat", "ai-embedding", "ai-health"],
                section_aliases: &[],
            },
            PageKind::Voice => &PageMeta {
                category: PageCategory::AiVoice,
                title_key: "voice",
                description_key: "page-voice-description",
                aliases: &[
                    "tts",
                    "stt",
                    "vad",
                    "speech",
                    "mic",
                    "microphone",
                    "kokoro",
                    "whisper",
                ],
                sections: &["voice-tts", "voice-stt", "voice-mic"],
                section_aliases: &[
                    SectionAlias {
                        alias: "tts",
                        section: "voice-tts",
                    },
                    SectionAlias {
                        alias: "kokoro",
                        section: "voice-tts",
                    },
                    SectionAlias {
                        alias: "stt",
                        section: "voice-stt",
                    },
                    SectionAlias {
                        alias: "whisper",
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
                category: PageCategory::AiVoice,
                title_key: "features",
                description_key: "page-features-description",
                aliases: &["proactive", "mind", "tools", "capability"],
                sections: &["features-capabilities", "features-mind", "features-tools"],
                section_aliases: &[],
            },
            PageKind::Accessibility => &PageMeta {
                category: PageCategory::Basics,
                title_key: "accessibility",
                description_key: "page-accessibility-description",
                aliases: &["spotlight", "caption", "overlay", "subtitle"],
                sections: &["accessibility-overlays"],
                section_aliases: &[],
            },
            PageKind::Memory => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "memory",
                description_key: "page-memory-description",
                aliases: &["journal", "recall", "commitment"],
                sections: &[
                    "memory-browse",
                    "memory-recall",
                    "memory-pending",
                    "memory-commitments",
                ],
                section_aliases: &[],
            },
            PageKind::MemoryLedger => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "memory-ledger",
                description_key: "page-memory-ledger-description",
                aliases: &["ledger", "journal", "delete", "edit"],
                sections: &["ledger-browse", "ledger-commitments"],
                section_aliases: &[],
            },
            PageKind::Permissions => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "permissions",
                description_key: "page-permissions-description",
                aliases: &["tools", "approval", "grant", "revoke"],
                sections: &["permissions-pending", "permissions-grants"],
                section_aliases: &[],
            },
            PageKind::Approvals => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "approvals",
                description_key: "page-approvals-description",
                aliases: &["plugin", "policy", "sandbox", "broker"],
                sections: &["approvals-policy"],
                section_aliases: &[],
            },
            PageKind::Connectors => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "connectors",
                description_key: "page-connectors-description",
                aliases: &["accounts", "service", "oauth"],
                sections: &["connectors-list", "connectors-detail"],
                section_aliases: &[],
            },
            PageKind::Sessions => &PageMeta {
                category: PageCategory::DataAccess,
                title_key: "sessions",
                description_key: "page-sessions-description",
                aliases: &["archive", "export", "import", "history"],
                sections: &["sessions-list", "sessions-search", "sessions-import"],
                section_aliases: &[],
            },
            PageKind::Debug => &PageMeta {
                category: PageCategory::System,
                title_key: "debug",
                description_key: "page-debug-description",
                aliases: &["overlay", "fps", "mask", "collider"],
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
    rank: u8,
}

/// Ranked page/section search over localized titles, descriptions,
/// technical aliases (TTS/STT/VAD), and section names.
fn compute_search(query: &str) -> Vec<SearchHit> {
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
                    rank: 1,
                });
            }
        }
    }
    hits.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.title.cmp(&b.title)));
    hits.truncate(16);
    hits
}

const fn use_compact_navigation(width: f32) -> bool {
    width < NARROW_NAV_THRESHOLD
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
    pub animation: AnimationControl,
    pub emotion_queue: EmotionQueue,
    /// When the runtime was constructed. Used for `now_secs` in
    /// emotion-queue timestamps.
    pub started_at: Instant,
    /// Settings search box text; results are recomputed when it changes.
    pub search_query: String,
    search_computed_for: String,
    search_hits: Vec<SearchHit>,
    /// Consumed on the next frame to switch page and highlight a section.
    pending_focus: Option<(PageKind, Option<&'static str>)>,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self {
            current_page: PageKind::Character,
            input: SettingsInputState::new(),
            animation: AnimationControl::new(),
            emotion_queue: EmotionQueue::default(),
            started_at: Instant::now(),
            search_query: String::new(),
            search_computed_for: String::new(),
            search_hits: Vec::new(),
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
        if let Some(page) = ui_state.focused_page {
            self.current_page = page;
        }
    }

    /// Render the full settings window. The caller is expected to
    /// have already opened an egui pass and supplied a `Ui` (via
    /// `egui::CentralPanel::show_inside`).
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut CharacterSettings,
        ai: Option<&Arc<AiBridge>>,
        world: &mut World,
        ui_entity: Entity,
        now_secs: f64,
    ) {
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

        // Consume a one-shot page focus request from tray / onboarding.
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
        let mut selected_page: Option<PageKind> = None;
        let narrow = use_compact_navigation(ui.available_width());
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
    }

    fn recompute_search(&mut self) {
        if self.search_query == self.search_computed_for {
            return;
        }
        self.search_computed_for = self.search_query.clone();
        self.search_hits = compute_search(&self.search_query);
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
        ai: &Arc<AiBridge>,
        world: &mut World,
        ui_entity: Entity,
        now_secs: f64,
        _selected_page: &mut Option<PageKind>,
    ) {
        let title = self.current_page.title();
        let description = self.current_page.description();
        components::page_header(ui, &title, &description);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("settings_page_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| match self.current_page {
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
                PageKind::Graphics => {
                    page_graphics::render(ui, settings, &mut self.animation, ai, world, ui_entity);
                }
                PageKind::Ai => page_ai::render(
                    ui,
                    settings,
                    &mut self.animation,
                    ai,
                    &mut self.input,
                    world,
                    ui_entity,
                ),
                PageKind::Voice => page_voice::render(ui, settings, ai, &mut self.input, world),
                PageKind::Features => {
                    page_features::render(ui, settings, ai, &mut self.input, world);
                }
                PageKind::Accessibility => page_accessibility::render(ui, settings),
                PageKind::Memory => page_memory::render(ui, ai, world, ui_entity),
                PageKind::MemoryLedger => page_memory_ledger::render(ui, ai, world, ui_entity),
                PageKind::Permissions => page_permissions::render(ui, ai, world, ui_entity),
                PageKind::Approvals => page_approvals::render(ui, settings, ai, world, ui_entity),
                PageKind::Connectors => page_connectors::render(ui, ai, world, ui_entity),
                PageKind::Sessions => page_sessions::render(ui, ai, world, ui_entity),
                PageKind::Debug => {
                    page_debug::render(ui, settings, &mut self.animation, ai, world, ui_entity);
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
        assert_eq!(PageKind::ALL.len(), 14);
        for (index, page) in PageKind::ALL.iter().enumerate() {
            assert!(PageKind::ALL[index + 1..].iter().all(|other| other != page));
        }
        let counts = PageCategory::ALL.map(|category| {
            PageKind::ALL
                .iter()
                .filter(|page| page.meta().category == category)
                .count()
        });
        assert_eq!(counts, [4, 3, 6, 1]);
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
            let hits = compute_search(query);
            assert_eq!(hits.first().and_then(|hit| hit.section), Some(expected));
            assert_eq!(hits.first().map(|hit| hit.page), Some(PageKind::Voice));
        }
    }

    #[test]
    fn search_ranks_page_titles_before_other_page_matches() {
        let hits = compute_search(&PageKind::Voice.title());
        let first = hits.first().unwrap();
        assert_eq!(first.page, PageKind::Voice);
        assert_eq!(first.section, None);
        assert_eq!(first.rank, 0);
    }

    #[test]
    fn search_ranks_section_aliases_before_page_aliases() {
        let hits = compute_search("tts");
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
        assert!(compute_search("").is_empty());
        assert!(compute_search("   ").is_empty());
    }

    #[test]
    fn compact_navigation_boundary_is_exclusive() {
        assert!(use_compact_navigation(719.0));
        assert!(!use_compact_navigation(720.0));
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
