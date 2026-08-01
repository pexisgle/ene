//! Spotlight quick-launcher overlay (Alt+Space) and floating
//! caption overlay.
//!
//! Both are rendered as egui `Window` popups inside the settings
//! window's egui context.

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::settings_ui::PageKind;
use std::sync::Arc;

// ── Spotlight data types ──

/// A single selectable action in the spotlight command palette.
#[derive(Debug, Clone)]
pub enum SpotlightAction {
    /// Generic labelled action.
    RunCommand { label: String },
    /// Open a settings page.
    OpenSettings { label: String, page: PageKind },
    /// Open the dedicated chat window.
    OpenChat { label: String },
    /// Pre-populate the chat input with a quick message.
    QuickChat { label: String },
    /// Toggle a named feature.
    ToggleFeature { label: String, feature: String },
}

impl SpotlightAction {
    /// Human-readable label displayed in the spotlight list.
    pub fn label(&self) -> &str {
        match self {
            Self::RunCommand { label }
            | Self::OpenSettings { label, .. }
            | Self::OpenChat { label }
            | Self::QuickChat { label }
            | Self::ToggleFeature { label, .. } => label,
        }
    }
}

/// Build the built-in set of spotlight actions.
pub fn default_actions() -> Vec<SpotlightAction> {
    vec![
        SpotlightAction::OpenSettings {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-open-settings"),
            page: PageKind::Ai,
        },
        SpotlightAction::OpenChat {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-open-chat"),
        },
        SpotlightAction::ToggleFeature {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-toggle-mic"),
            feature: "mic".to_string(),
        },
        SpotlightAction::QuickChat {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-quick-chat"),
        },
        SpotlightAction::RunCommand {
            label: i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-action-toggle-caption"),
        },
    ]
}

/// Filter the actions list by a query string (substring match on label,
/// case-insensitive).
pub fn filter_actions<'a>(query: &str, actions: &'a [SpotlightAction]) -> Vec<&'a SpotlightAction> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return actions.iter().collect();
    }
    actions
        .iter()
        .filter(|a| a.label().to_lowercase().contains(&q))
        .collect()
}

// ── Spotlight overlay render ──

/// Render the spotlight quick-launcher as a centered egui `Window`.
///
/// Reads the current UI state to determine visibility and input;
/// writes back changes (updated input, selection, dismiss) via bevy
/// world messages.
pub fn render_spotlight_overlay(
    ctx: &egui::Context,
    _ai: Option<&Arc<AiBridge>>,
    world: &mut World,
    ui_entity: Entity,
) {
    // Snapshot current state (drop borrow before Window::show).
    let snapshot = {
        let Some(state) = world.get::<UiStateComponent>(ui_entity) else {
            return;
        };
        if !state.0.spotlight_visible {
            return;
        }
        (
            state.0.spotlight_input.clone(),
            state.0.spotlight_selection,
            state.0.spotlight_visible,
        )
    };
    let (input, selection, _visible) = snapshot;

    let actions = default_actions();
    let title = i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-title");
    let placeholder = i18n_embed_fl::fl!(crate::i18n::loader(), "spotlight-placeholder");

    let mut input_buf = input;
    let mut selected_idx = selection;
    let mut close = false;
    let mut execute = false;

    egui::Window::new(title)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -80.0])
        .collapsible(false)
        .resizable(false)
        .fixed_size([420.0, 340.0])
        .title_bar(true)
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                let text_response = ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut input_buf)
                        .hint_text(placeholder)
                        .desired_width(f32::INFINITY),
                );

                if text_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }

                ui.separator();

                let filtered_now = filter_actions(&input_buf, &actions);
                let count = filtered_now.len();
                let sel = selected_idx.min(count.saturating_sub(1));

                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (i, action) in filtered_now.iter().enumerate() {
                            let is_selected = i == sel;
                            let label = action.label();

                            let response = ui.selectable_label(is_selected, label);

                            if response.clicked() {
                                execute = true;
                                selected_idx = i;
                            }

                            if is_selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });

                if count == 0 && !input_buf.is_empty() {
                    ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "cancel"));
                }
            });
        });

    // ── Post-render: sync back to world ──
    // Resolve the action to execute (if any) before borrowing the world
    // mutably, so the immutable borrow of `actions` is dropped first.
    let action_to_execute = if execute {
        let filtered_now = filter_actions(&input_buf, &actions);
        filtered_now.get(selected_idx).map(|a| (*a).clone())
    } else {
        None
    };

    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.spotlight_input.clone_from(&input_buf);
        state.0.spotlight_selection = selected_idx;
        if execute || close {
            state.0.spotlight_visible = false;
        }
    }

    if let Some(action) = action_to_execute {
        execute_spotlight_action(&action, world, ui_entity);
    }
}

/// Dispatch a selected spotlight action via ECS messages.
fn execute_spotlight_action(action: &SpotlightAction, world: &mut World, ui_entity: Entity) {
    match action {
        SpotlightAction::OpenSettings { page, .. } => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.settings_window_visible = true;
                state.0.focused_page = Some(*page);
            }
        }
        SpotlightAction::OpenChat { .. } | SpotlightAction::QuickChat { .. } => {
            world.write_message(crate::event::chat::OpenChat);
        }
        SpotlightAction::ToggleFeature { feature, .. } => {
            if feature == "mic" {
                // TODO: wire the mic toggle from spotlight via EventChannels
                // once the mic toggle is connected to the audio subsystem.
            }
        }
        SpotlightAction::RunCommand { .. } => {
            // The built-in command toggles the floating caption overlay,
            // giving the caption window a launch path.
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.caption_visible = !state.0.caption_visible;
            }
        }
    }
}

// ── Caption overlay render ──

/// Render the floating caption overlay as a semi-transparent egui `Window`.
pub fn render_caption_overlay(ctx: &egui::Context, world: &mut World, ui_entity: Entity) {
    let snapshot = {
        let Some(state) = world.get::<UiStateComponent>(ui_entity) else {
            return;
        };
        if !state.0.caption_visible {
            return;
        }
        (state.0.caption_text.clone(), state.0.caption_position)
    };
    let (text, pos) = snapshot;

    let title = i18n_embed_fl::fl!(crate::i18n::loader(), "caption-title");
    let mut close = false;

    let window = egui::Window::new(title)
        .default_pos(egui::pos2(pos.0, pos.1))
        .collapsible(false)
        .resizable(true)
        .fixed_size([360.0, 120.0])
        .title_bar(true);

    let response = window.show(ctx, |ui| {
        egui::Frame {
            fill: egui::Color32::from_black_alpha(180),
            ..Default::default()
        }
        .show(ui, |ui| {
            ui.set_min_height(80.0);
            ui.label(&text);
        });

        ui.horizontal(|ui| {
            if ui
                .button(i18n_embed_fl::fl!(crate::i18n::loader(), "close"))
                .clicked()
            {
                close = true;
            }
        });
    });

    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        if close {
            state.0.caption_visible = false;
        }
        // Persist the window's current position so it stays where
        // the user dragged it.
        if let Some(response) = response {
            state.0.caption_position = (response.response.rect.min.x, response.response.rect.min.y);
        }
    }
}
