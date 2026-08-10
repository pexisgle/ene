//! Debug settings page.
//!
//! Toggles for raycast colliders, input region debug outlines, the Linux
//! mask overlay, and read-only pipeline diagnostics (update FPS,
//! downsample factor).
use super::components::{section_card, setting_row, toggle_row};
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
        section_card(
            ui,
            "debug-overlays",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-debug-overlays"),
            |ui| {
                let collider_on = collider_debug_on(world, ui_entity);
                let mut collider_check = collider_on;
                if toggle_row(
                    ui,
                    "debug_collider",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "raycast-colliders"),
                    &if collider_on {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "visible-f3")
                    } else {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "hidden-f3")
                    },
                    &mut collider_check,
                ) && collider_check != collider_on
                {
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
                if collider_on {
                    setting_row(
                        ui,
                        "debug_hovered_bone",
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "hovered-bone"),
                        "",
                        |ui| {
                            let name = world
                                .get::<UiStateComponent>(ui_entity)
                                .and_then(|state| state.0.hovered_bone_name.clone());
                            ui.label(name.unwrap_or_else(|| {
                                i18n_embed_fl::fl!(crate::i18n::loader(), "none")
                            }));
                        },
                    );
                }

                let input_on = input_region_debug_on(world, ui_entity);
                let mut input_check = input_on;
                if toggle_row(
                    ui,
                    "debug_input_region",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "input-region-debug"),
                    &if input_on {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "visible-f9")
                    } else {
                        i18n_embed_fl::fl!(crate::i18n::loader(), "hidden-f9")
                    },
                    &mut input_check,
                ) && input_check != input_on
                {
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

                #[cfg(target_os = "linux")]
                {
                    let overlay_on = world
                        .get::<UiStateComponent>(ui_entity)
                        .is_some_and(|state| state.0.debug_overlay_visible);
                    let mut overlay_check = overlay_on;
                    if toggle_row(
                        ui,
                        "debug_mask_overlay",
                        &i18n_embed_fl::fl!(crate::i18n::loader(), "mask-overlay-debug"),
                        &if overlay_on {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "visible")
                        } else {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "hidden")
                        },
                        &mut overlay_check,
                    ) && overlay_check != overlay_on
                    {
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
                }
            },
        );

        section_card(
            ui,
            "debug-pipeline",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-debug-pipeline"),
            |ui| {
                setting_row(
                    ui,
                    "debug_update_fps",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "debug-update-fps"),
                    "",
                    |ui| {
                        let resolved = settings.graphics().resolved();
                        ui.label(crate::settings::debug_fps_label(
                            settings.language(),
                            resolved.debug_fps,
                        ));
                    },
                );
                #[cfg(target_os = "linux")]
                setting_row(
                    ui,
                    "debug_mask_downsample",
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "mask-downsample"),
                    "",
                    |ui| {
                        let resolved = settings.graphics().resolved();
                        ui.label(format!("{}x", resolved.mask_render_downsample));
                    },
                );
            },
        );
    });
}

fn collider_debug_on(world: &World, ui_entity: Entity) -> bool {
    world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|state| state.0.show_collider_debug)
}

fn input_region_debug_on(world: &World, ui_entity: Entity) -> bool {
    world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|state| state.0.show_input_region_debug)
}
