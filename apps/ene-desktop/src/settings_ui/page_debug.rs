//! Debug settings page.
//!
//! Handles toggling raycast colliders, input region debug outlines,
//! throttling update rates via debug FPS, and (Linux only) the mask
//! overlay and downsampling factor.
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::settings::CharacterSettings;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label("Raycast Colliders (Debug)");
            let debug_on = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.show_collider_debug
            } else {
                false
            };
            let mut checkbox = debug_on;
            if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_on {
                apply_action(
                    SettingsAction::ToggleColliderDebug,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if checkbox {
                    "Visible (F3)"
                } else {
                    "Hidden (F3)"
                }),
            );
        });

        let show_colliders = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity)
        {
            ui_state.show_collider_debug
        } else {
            false
        };
        if show_colliders {
            ui.horizontal(|ui| {
                ui.label("Hovered Bone");
                let name = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                    ui_state.hovered_bone_name.clone()
                } else {
                    None
                };
                ui.label(name.as_deref().unwrap_or("None"));
            });
        }

        ui.horizontal(|ui| {
            ui.label("Input Region (Debug)");
            let debug_on = if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.show_input_region_debug
            } else {
                false
            };
            let mut checkbox = debug_on;
            if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_on {
                apply_action(
                    SettingsAction::ToggleInputRegionDebug,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(if checkbox {
                    "Visible (F9)"
                } else {
                    "Hidden (F9)"
                }),
            );
        });

        ui.horizontal(|ui| {
            ui.label("Debug Update FPS");
            if ui.button("<").clicked() {
                apply_action(
                    SettingsAction::DebugFpsDown,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
            ui.add_sized(
                [220.0, 0.0],
                egui::Label::new(crate::settings::debug_fps_label(
                    settings.graphics.debug_fps,
                )),
            );
            if ui.button(">").clicked() {
                apply_action(
                    SettingsAction::DebugFpsUp,
                    settings,
                    animation,
                    ai,
                    world,
                    ui_entity,
                    None,
                    0.0,
                );
            }
        });

        render_linux_only(ui, settings, animation, ai, world, ui_entity);
    });
}

#[cfg(target_os = "linux")]
fn render_linux_only(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut hecs::World,
    ui_entity: hecs::Entity,
) {
    ui.horizontal(|ui| {
        ui.label("Mask Overlay (Debug)");
        let debug_overlay_visible =
            if let Ok(ui_state) = world.get::<&crate::settings::UiState>(ui_entity) {
                ui_state.debug_overlay_visible
            } else {
                false
            };
        let mut checkbox = debug_overlay_visible;
        if ui.checkbox(&mut checkbox, "").changed() && checkbox != debug_overlay_visible {
            apply_action(
                SettingsAction::ToggleDebugOverlay,
                settings,
                animation,
                ai,
                world,
                ui_entity,
                None,
                0.0,
            );
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(if checkbox { "Visible" } else { "Hidden" }),
        );
    });

    ui.horizontal(|ui| {
        ui.label("Mask Downsample");
        if ui.button("<").clicked() {
            apply_action(
                SettingsAction::MaskDownsampleDown,
                settings,
                animation,
                ai,
                world,
                ui_entity,
                None,
                0.0,
            );
        }
        ui.add_sized(
            [220.0, 0.0],
            egui::Label::new(format!("{}x", settings.graphics.mask_render_downsample)),
        );
        if ui.button(">").clicked() {
            apply_action(
                SettingsAction::MaskDownsampleUp,
                settings,
                animation,
                ai,
                world,
                ui_entity,
                None,
                0.0,
            );
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn render_linux_only(
    _ui: &mut egui::Ui,
    _settings: &mut CharacterSettings,
    _animation: &mut AnimationControl,
    _ai: &Arc<AiBridge>,
    _world: &mut hecs::World,
    _ui_entity: hecs::Entity,
) {
}
