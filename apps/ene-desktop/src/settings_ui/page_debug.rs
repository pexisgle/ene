//! Debug settings page.
//!
//! Handles toggling raycast colliders, input region debug outlines,
//! throttling update rates via debug FPS, and (Linux only) the mask
//! overlay and downsampling factor.
use super::widgets::{SettingsAction, apply_action};
use crate::ai_bridge::AiBridge;
use crate::character_state::AnimationControl;
use crate::component::ui::UiStateComponent;
use crate::settings::CharacterSettings;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::sync::Arc;

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "raycast-colliders"
            ));
            let debug_on = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                ui_state.0.show_collider_debug
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
                    i18n_embed_fl::fl!(crate::i18n::loader(), "visible-f3")
                } else {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "hidden-f3")
                }),
            );
        });

        let show_colliders = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
            ui_state.0.show_collider_debug
        } else {
            false
        };
        if show_colliders {
            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "hovered-bone"));
                let name = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                    ui_state.0.hovered_bone_name.clone()
                } else {
                    None
                };
                ui.label(name.unwrap_or_else(|| i18n_embed_fl::fl!(crate::i18n::loader(), "none")));
            });
        }

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "input-region-debug"
            ));
            let debug_on = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity) {
                ui_state.0.show_input_region_debug
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
                    i18n_embed_fl::fl!(crate::i18n::loader(), "visible-f9")
                } else {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "hidden-f9")
                }),
            );
        });

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "debug-update-fps"
            ));
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
                    settings.language,
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
    world: &mut World,
    ui_entity: Entity,
) {
    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "mask-overlay-debug"
        ));
        let debug_overlay_visible = if let Some(ui_state) = world.get::<UiStateComponent>(ui_entity)
        {
            ui_state.0.debug_overlay_visible
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
            egui::Label::new(if checkbox {
                i18n_embed_fl::fl!(crate::i18n::loader(), "visible")
            } else {
                i18n_embed_fl::fl!(crate::i18n::loader(), "hidden")
            }),
        );
    });

    ui.horizontal(|ui| {
        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "mask-downsample"));
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
    _world: &mut World,
    _ui_entity: Entity,
) {
}
