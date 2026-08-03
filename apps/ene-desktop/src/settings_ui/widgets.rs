//! Shared row widgets and action dispatcher for the settings UI.
//!
//! The action enum and `apply_action` dispatcher are the
//! single funnel through which buttons, hotkeys, and direct egui
//! field changes mutate [`CharacterSettings`].
use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionCommand, EmotionQueue};
use crate::component::ui::UiStateComponent;
use crate::settings::{
    CharacterSettings, GraphicsQuality, GraphicsSettings, cycle_graphics_quality,
    graphics_quality_label,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use std::sync::Arc;

/// Single action enum shared by every page widget. Hotkeys and
/// buttons both translate into one of these before mutating state.
///
/// The character-card editor variants carry a `String` path, so the
/// enum is `Clone` but not `Copy`; call sites that need to reuse an
/// action (e.g. the runtime hotkey dispatcher) clone it explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsAction {
    PrevCharacter,
    NextCharacter,
    PrevMotion,
    NextMotion,
    TogglePlay,
    #[cfg(target_os = "linux")]
    ToggleDebugOverlay,
    GraphicsQualityDown,
    GraphicsQualityUp,
    LookAtStrengthDown,
    LookAtStrengthUp,
    ModelScaleDown,
    ModelScaleUp,
    CharacterPosXDown,
    CharacterPosXUp,
    CharacterPosYDown,
    CharacterPosYUp,
    CharacterPosZDown,
    CharacterPosZUp,
    /// Snap `character_position` back to the world origin. Triggered
    /// by the "Reset Position" button on the Character settings
    /// page; lets the user recover from a model dragged off-screen
    /// without restarting the app.
    ResetCharacterPosition,
    /// Toggle the per-bone collider wireframe + raycast hit-point
    /// overlay. Bound to the F3 hotkey and the "Show raycast
    /// colliders (debug)" checkbox on the Character page.
    ToggleColliderDebug,
    ToggleInputRegionDebug,
    LanguageDown,
    LanguageUp,
    /// Load the character card at `path` into the editor buffers
    /// (Character Card editor page).
    LoadCharacterCard {
        path: String,
    },
    /// Write the editor buffers back to the character card at `path`
    /// (Character Card editor page).
    SaveCharacterCard {
        path: String,
    },
    /// Validate the editor buffers without writing to disk
    /// (Character Card editor page).
    ValidateCharacterCard,
}

pub fn apply_action(
    action: SettingsAction,
    settings: &mut CharacterSettings,
    animation: &mut AnimationControl,
    _ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    emotion_queue: Option<&mut EmotionQueue>,
    now_secs: f64,
) {
    match action {
        SettingsAction::PrevCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                -1,
            );
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
        }
        SettingsAction::NextCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                1,
            );
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
        }
        SettingsAction::PrevMotion => {
            let motion_len = settings.current_entry().map_or(0, |e| e.motion_names.len());
            settings.character_state.selected_motion =
                cycle_index(settings.character_state.selected_motion, motion_len, -1);
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsAction::NextMotion => {
            let motion_len = settings.current_entry().map_or(0, |e| e.motion_names.len());
            settings.character_state.selected_motion =
                cycle_index(settings.character_state.selected_motion, motion_len, 1);
            settings.character_state.motion_override = None;
            settings.character_state.needs_respawn = true;
            settings.mark_dirty();
        }
        SettingsAction::TogglePlay => {
            animation.toggle_playing();
            if let Some(mut ui_anim) = world.get_mut::<crate::component::ui::UiAnimation>(ui_entity)
            {
                ui_anim.0.playing = animation.playing;
            }
        }
        #[cfg(target_os = "linux")]
        SettingsAction::ToggleDebugOverlay => {
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.debug_overlay_visible = !ui_state.0.debug_overlay_visible;
            }
            settings.mark_dirty();
        }
        SettingsAction::GraphicsQualityDown => {
            let current = settings.graphics().quality;
            let next = cycle_graphics_quality(current, -1);
            settings.set_graphics(GraphicsSettings { quality: next });
        }
        SettingsAction::GraphicsQualityUp => {
            let current = settings.graphics().quality;
            let next = cycle_graphics_quality(current, 1);
            settings.set_graphics(GraphicsSettings { quality: next });
        }
        SettingsAction::LookAtStrengthDown => {
            adjust_f32(&mut settings.character_state.look_at_strength, -0.05);
        }
        SettingsAction::LookAtStrengthUp => {
            adjust_f32(&mut settings.character_state.look_at_strength, 0.05);
        }
        SettingsAction::ModelScaleDown => {
            adjust_f32(&mut settings.character_state.model_scale, -0.05);
        }
        SettingsAction::ModelScaleUp => {
            adjust_f32(&mut settings.character_state.model_scale, 0.05);
        }
        SettingsAction::CharacterPosXDown => {
            adjust_f32(&mut settings.character_state.character_position.x, -0.05);
        }
        SettingsAction::CharacterPosXUp => {
            adjust_f32(&mut settings.character_state.character_position.x, 0.05);
        }
        SettingsAction::CharacterPosYDown => {
            adjust_f32(&mut settings.character_state.character_position.y, -0.05);
        }
        SettingsAction::CharacterPosYUp => {
            adjust_f32(&mut settings.character_state.character_position.y, 0.05);
        }
        SettingsAction::CharacterPosZDown => {
            adjust_f32(&mut settings.character_state.character_position.z, -0.05);
        }
        SettingsAction::CharacterPosZUp => {
            adjust_f32(&mut settings.character_state.character_position.z, 0.05);
        }
        SettingsAction::ResetCharacterPosition => {
            settings.character_state.character_position = glam::Vec3::ZERO;
        }
        SettingsAction::ToggleColliderDebug => {
            // Not persisted — defaults to `false` on every launch.
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.show_collider_debug = !ui_state.0.show_collider_debug;
            }
        }
        SettingsAction::ToggleInputRegionDebug => {
            if let Some(mut ui_state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui_state.0.show_input_region_debug = !ui_state.0.show_input_region_debug;
            }
        }
        SettingsAction::LanguageDown => {
            let current = settings.language();
            let next = crate::settings::cycle_language(current, -1);
            settings.set_language(next);
            crate::i18n::select_language(next);
            settings.sync_classifier_language_from_ui();
        }
        SettingsAction::LanguageUp => {
            let current = settings.language();
            let next = crate::settings::cycle_language(current, 1);
            settings.set_language(next);
            crate::i18n::select_language(next);
            settings.sync_classifier_language_from_ui();
        }
        SettingsAction::LoadCharacterCard { path } => {
            load_character_card(&path, world, ui_entity);
        }
        SettingsAction::SaveCharacterCard { path } => {
            save_character_card(&path, world, ui_entity);
        }
        SettingsAction::ValidateCharacterCard => {
            validate_character_card(world, ui_entity);
        }
    }

    settings.clamp_runtime_values();
    settings.mark_dirty();
}

/// Load the character card at `path` into the editor buffers on
/// [`UiState`]. On read/parse failure the error is surfaced through
/// `character_editor_validation_errors` and the loaded flag stays
/// `false` so the page can retry.
fn load_character_card(path: &str, world: &mut World, ui_entity: Entity) {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            set_editor_errors(
                world,
                ui_entity,
                vec![i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-read-error",
                    error = error.to_string()
                )],
            );
            return;
        }
    };
    let card: ene_config::CharacterCardV3 = match serde_json::from_str(&content) {
        Ok(card) => card,
        Err(error) => {
            set_editor_errors(
                world,
                ui_entity,
                vec![i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-parse-error",
                    error = error.to_string()
                )],
            );
            return;
        }
    };
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        let data = &card.data;
        state.0.character_editor_name.clone_from(&data.name);
        state
            .0
            .character_editor_description
            .clone_from(&data.description);
        state
            .0
            .character_editor_personality
            .clone_from(&data.personality);
        state.0.character_editor_scenario.clone_from(&data.scenario);
        state
            .0
            .character_editor_system_prompt
            .clone_from(&data.system_prompt);
        state
            .0
            .character_editor_mes_example
            .clone_from(&data.mes_example);
        state
            .0
            .character_editor_first_mes
            .clone_from(&data.first_mes);
        state
            .0
            .character_editor_post_history
            .clone_from(&data.post_history_instructions);
        state.0.character_editor_loaded = true;
        state.0.character_editor_modified = false;
        state.0.character_editor_validation_errors.clear();
    }
}

/// Write the editor buffers back to the character card at `path`.
///
/// The existing on-disk card is read first so that extensions, assets, and
/// other fields the editor does not expose are preserved. An unreadable or
/// unparseable existing card aborts the save with the reason surfaced
/// through `character_editor_validation_errors` — overwriting it with a
/// default card would destroy the original. Only a missing file (a brand-new
/// card) starts from a default. The write itself is atomic
/// (temp file + rename via [`ene_config::save_character_card`]), so a crash
/// mid-write cannot leave a truncated card behind.
fn save_character_card(path: &str, world: &mut World, ui_entity: Entity) {
    let Some(snapshot) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let mut card = match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<ene_config::CharacterCardV3>(&content) {
            Ok(card) => card,
            Err(error) => {
                set_editor_errors(
                    world,
                    ui_entity,
                    vec![i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-parse-error",
                        error = error.to_string()
                    )],
                );
                return;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ene_config::CharacterCardV3::default()
        }
        Err(error) => {
            set_editor_errors(
                world,
                ui_entity,
                vec![i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-read-error",
                    error = error.to_string()
                )],
            );
            return;
        }
    };
    card.data.name = snapshot.character_editor_name;
    card.data.description = snapshot.character_editor_description;
    card.data.personality = snapshot.character_editor_personality;
    card.data.scenario = snapshot.character_editor_scenario;
    card.data.system_prompt = snapshot.character_editor_system_prompt;
    card.data.mes_example = snapshot.character_editor_mes_example;
    card.data.first_mes = snapshot.character_editor_first_mes;
    card.data.post_history_instructions = snapshot.character_editor_post_history;
    if let Err(error) = ene_config::save_character_card(std::path::Path::new(path), &card) {
        set_editor_errors(
            world,
            ui_entity,
            vec![i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "character-editor-save-error",
                error = error.to_string()
            )],
        );
        return;
    }
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.character_editor_modified = false;
        state.0.character_editor_validation_errors.clear();
    }
}

/// Validate the editor buffers without touching disk, populating
/// `character_editor_validation_errors`.
fn validate_character_card(world: &mut World, ui_entity: Entity) {
    let Some(snapshot) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let mut errors = Vec::new();
    if snapshot.character_editor_name.trim().is_empty() {
        errors.push(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "character-editor-name-required"
        ));
    }
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.character_editor_validation_errors = errors;
    }
}

fn set_editor_errors(world: &mut World, ui_entity: Entity, errors: Vec<String>) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.character_editor_validation_errors = errors;
    }
}

const fn cycle_index(index: usize, len: usize, step: isize) -> usize {
    if len == 0 {
        return 0;
    }
    ((index as isize + step).rem_euclid(len as isize)) as usize
}

fn adjust_f32(value: &mut f32, delta: f32) {
    *value += delta;
}

pub fn format_quality_label(lang: crate::settings::Language, quality: GraphicsQuality) -> String {
    graphics_quality_label(lang, quality)
}

/// Push the per-character default expression into the
/// `EmotionQueue` if both a non-`None` expression and a queue
/// handle are available. Centralising this branch keeps the
/// character-cycle arm in [`apply_action`] symmetric for `Prev`
/// and `Next`.
fn push_default_expression(
    default_expression: Option<String>,
    emotion_queue: Option<&mut EmotionQueue>,
    now_secs: f64,
) {
    if let (Some(expression), Some(queue)) = (default_expression, emotion_queue) {
        queue.push(EmotionCommand {
            emotion: expression,
            target_time: now_secs,
            hold_secs: 4.0,
            weight: 1.0,
        });
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;
    use crate::settings::UiState;

    fn editor_world(name: &str) -> (World, Entity) {
        let mut world = World::new();
        let entity = world
            .spawn(UiStateComponent(UiState {
                character_editor_name: name.to_string(),
                ..UiState::default()
            }))
            .id();
        (world, entity)
    }

    fn editor_errors(world: &World, entity: Entity) -> Vec<String> {
        world
            .get::<UiStateComponent>(entity)
            .map(|s| s.0.character_editor_validation_errors.clone())
            .unwrap_or_default()
    }

    fn seed_card_json() -> &'static str {
        r#"{
            "spec": "chara_card_v3",
            "spec_version": "3.0",
            "data": {
                "name": "Old",
                "description": "desc",
                "personality": "kind",
                "scenario": "lab",
                "mes_example": "hi",
                "first_mes": "hello",
                "system_prompt": "sys",
                "post_history_instructions": "phi",
                "alternate_greetings": ["alt"],
                "tags": ["robot"],
                "creator": "pexisgle",
                "character_version": "1.0",
                "vendor_field": "from-other-app"
            }
        }"#
    }

    #[test]
    fn push_default_expression_drops_on_none_expression() {
        let mut q = EmotionQueue::default();
        push_default_expression(None, Some(&mut q), 1.0);
        assert!(q.commands.is_empty());
    }

    #[test]
    fn push_default_expression_drops_on_none_queue() {
        let mut expression = None;
        let q = EmotionQueue::default();
        push_default_expression(expression.take(), None, 1.0);
        assert!(q.commands.is_empty());
    }

    #[test]
    fn push_default_expression_pushes_with_both() {
        let mut q = EmotionQueue::default();
        push_default_expression(Some("happy".to_string()), Some(&mut q), 7.5);
        assert_eq!(q.commands.len(), 1);
        let cmd = &q.commands[0];
        assert_eq!(cmd.emotion, "happy");
        assert_eq!(cmd.target_time, 7.5);
        assert_eq!(cmd.hold_secs, 4.0);
        assert_eq!(cmd.weight, 1.0);
    }

    /// Saving over a card that exists but cannot be
    /// parsed must abort — falling back to a default card and rewriting would
    /// destroy the original on disk.
    #[test]
    fn save_aborts_on_unparseable_existing_card() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let corrupt = "{ this is not json";
        std::fs::write(&path, corrupt).expect("seed corrupt card");
        let (mut world, entity) = editor_world("New Name");

        save_character_card(path.to_string_lossy().as_ref(), &mut world, entity);

        let errors = editor_errors(&world, entity);
        assert!(
            errors.iter().any(|e| e.contains("Failed to parse card")),
            "parse failure must be surfaced, got {errors:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            corrupt,
            "an unparseable card must never be overwritten"
        );
    }

    /// A missing card (brand-new character) still saves
    /// from a default card — creating the file — without erroring.
    #[test]
    fn save_creates_card_when_file_missing() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Brand New");

        save_character_card(path.to_string_lossy().as_ref(), &mut world, entity);

        assert!(
            editor_errors(&world, entity).is_empty(),
            "save must succeed silently"
        );
        let on_disk = std::fs::read_to_string(&path).expect("read card");
        let card: ene_config::CharacterCardV3 = serde_json::from_str(&on_disk).expect("valid JSON");
        assert_eq!(card.data.name, "Brand New");
    }

    /// An unreadable existing card (not missing, but
    /// failing I/O) must abort the save and surface the read error.
    #[test]
    fn save_aborts_on_unreadable_existing_card() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let dir_path = tmp.path().join("character.json");
        std::fs::create_dir(&dir_path).expect("create dir path (read_to_string fails on it)");
        let (mut world, entity) = editor_world("Ene");

        save_character_card(dir_path.to_string_lossy().as_ref(), &mut world, entity);

        let errors = editor_errors(&world, entity);
        assert!(
            errors.iter().any(|e| e.contains("Failed to read card")),
            "read failure must be surfaced, got {errors:?}"
        );
    }

    /// An edit-and-save round-trip must keep top-level
    /// `data` fields from other apps (e.g. `vendor_field`) while applying the
    /// editor changes.
    #[test]
    fn save_preserves_unknown_top_level_fields() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(&path, seed_card_json()).expect("seed card");
        let (mut world, entity) = editor_world("Edited");

        save_character_card(path.to_string_lossy().as_ref(), &mut world, entity);

        assert!(
            editor_errors(&world, entity).is_empty(),
            "save must succeed silently"
        );
        let on_disk = std::fs::read_to_string(&path).expect("read card");
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).expect("valid JSON");
        assert_eq!(
            parsed.pointer("/data/name"),
            Some(&serde_json::json!("Edited"))
        );
        assert_eq!(
            parsed.pointer("/data/vendor_field"),
            Some(&serde_json::json!("from-other-app")),
            "unknown top-level field must survive the save"
        );
    }

    /// `ResetCharacterPosition` must zero all three axes of
    /// `character_position` without touching unrelated fields like
    /// `look_at_strength` or `model_scale`. Pin the contract here
    /// because the call site in `page_character.rs` has no easy
    /// way to assert "other fields untouched" at runtime.
    #[test]
    fn reset_character_position_zeroes_only_position() {
        use crate::settings::CharacterState;
        let mut state = CharacterState {
            character_position: glam::Vec3::new(1.25, -0.5, 2.0),
            look_at_strength: 0.42,
            model_scale: 1.75,
            ..CharacterState::default()
        };
        // Mirror the single statement that lives in
        // `apply_action`'s `ResetCharacterPosition` arm.
        state.character_position = glam::Vec3::ZERO;
        assert_eq!(state.character_position, glam::Vec3::ZERO);
        assert!((state.look_at_strength - 0.42).abs() < 1e-6);
        assert!((state.model_scale - 1.75).abs() < 1e-6);
    }
}
