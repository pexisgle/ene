//! Shared row widgets and action dispatcher for the settings UI.
//!
//! The action enum and `apply_action` dispatcher are the
//! single funnel through which buttons, hotkeys, and direct egui
//! field changes mutate [`CharacterSettings`].
use crate::ai_bridge::AiBridge;
use crate::character_state::{AnimationControl, EmotionCommand, EmotionQueue};
use crate::component::ui::UiStateComponent;
use crate::settings::{
    CharacterSettings, EditorIssue, EditorSeverity, GraphicsQuality, GraphicsSettings, UiState,
    cycle_graphics_quality, graphics_quality_label,
};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_config::{
    CharacterCardV3, DEFAULT_VRM_PATH, DEFAULT_VRMA_PATH, EneAssetKind, ResolvedAssetUri,
    resolve_asset_uri,
};
use std::path::{Path, PathBuf};
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
        assets_dir: String,
    },
    /// Validate the editor buffers without writing to disk
    /// (Character Card editor page).
    ValidateCharacterCard {
        card_path: String,
        assets_dir: String,
    },
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
            if defer_character_switch_if_unsaved(idx, world, ui_entity) {
                return;
            }
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.reset_character_editor();
            }
        }
        SettingsAction::NextCharacter => {
            let idx = cycle_index(
                settings.character_state.selected_character,
                settings.characters.len(),
                1,
            );
            if defer_character_switch_if_unsaved(idx, world, ui_entity) {
                return;
            }
            push_default_expression(settings.select_character(idx), emotion_queue, now_secs);
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.reset_character_editor();
            }
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
        SettingsAction::SaveCharacterCard { path, assets_dir } => {
            save_character_card(&path, &assets_dir, world, ui_entity);
        }
        SettingsAction::ValidateCharacterCard {
            card_path,
            assets_dir,
        } => {
            validate_character_card(&card_path, &assets_dir, world, ui_entity);
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
    let Some(card) = read_editable_card(Path::new(path), world, ui_entity) else {
        return;
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
            .character_editor_creator_notes
            .clone_from(&data.creator_notes);
        state
            .0
            .character_editor_post_history
            .clone_from(&data.post_history_instructions);
        state
            .0
            .character_editor_alternate_greetings
            .clone_from(&data.alternate_greetings);
        state
            .0
            .character_editor_lorebook
            .clone_from(&data.character_book);
        state.0.character_editor_motion_catalog = data
            .extensions
            .ene
            .as_ref()
            .and_then(|ene| ene.motion_catalog.clone());
        state.0.character_editor_locale_diffs = sidecar_diffs(Path::new(path));
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
/// card) starts from a default. Validation runs before any write; `Error`
/// findings abort the save. The pre-save card is copied to `<name>.bak` once,
/// then the write goes through the atomic temp-file-and-rename
/// ([`ene_config::save_character_card`]) so a crash mid-write can never leave
/// a truncated card behind.
fn save_character_card(path: &str, assets_dir: &str, world: &mut World, ui_entity: Entity) {
    let Some(snapshot) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let path = Path::new(path);
    let Some(mut card) = read_editable_card(path, world, ui_entity) else {
        return;
    };
    card = build_card_from_buffers(&snapshot, card);

    let issues = validate_card(
        &card,
        path.parent().unwrap_or(Path::new(".")),
        Path::new(assets_dir),
    );
    if issues
        .iter()
        .any(|issue| issue.severity == EditorSeverity::Error)
    {
        set_editor_errors(world, ui_entity, issues);
        return;
    }

    if let Err(error) = backup_card(path) {
        set_editor_errors(
            world,
            ui_entity,
            vec![EditorIssue {
                location: card_file_label(path),
                message: i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-backup-error",
                    error = error.to_string()
                ),
                severity: EditorSeverity::Error,
            }],
        );
        return;
    }
    if let Err(error) = ene_config::save_character_card(path, &card) {
        set_editor_errors(
            world,
            ui_entity,
            vec![EditorIssue {
                location: card_file_label(path),
                message: i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-save-error",
                    error = error.to_string()
                ),
                severity: EditorSeverity::Error,
            }],
        );
        return;
    }
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.character_editor_modified = false;
        state.0.character_editor_validation_errors = issues;
    }
}

/// Validate the editor buffers without touching disk, populating
/// `character_editor_validation_errors`.
fn validate_character_card(
    card_path: &str,
    assets_dir: &str,
    world: &mut World,
    ui_entity: Entity,
) {
    let Some(snapshot) = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone())
    else {
        return;
    };
    let path = Path::new(card_path);
    let Some(card) = read_editable_card(path, world, ui_entity) else {
        return;
    };
    let card = build_card_from_buffers(&snapshot, card);
    let issues = validate_card(
        &card,
        path.parent().unwrap_or(Path::new(".")),
        Path::new(assets_dir),
    );
    set_editor_errors(world, ui_entity, issues);
}

fn set_editor_errors(world: &mut World, ui_entity: Entity, errors: Vec<EditorIssue>) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.character_editor_validation_errors = errors;
    }
}

/// Applies the discard dialog's "discard" choice to the editor state.
///
/// Returns `true` when a deferred character switch must still be applied to
/// settings (the caller releases the world borrow first). The close and
/// app-exit intents keep their request flags so the runtime completes them
/// once the dirty flag is gone.
pub(crate) fn apply_discard_decision(state: &mut UiState) -> bool {
    if state.character_editor_pending_character.is_some() {
        return true;
    }
    if state.character_editor_reload_pending {
        state.reset_character_editor();
        return false;
    }
    state.character_editor_modified = false;
    state.character_editor_loaded = false;
    state.character_editor_validation_errors.clear();
    false
}

/// Records a deferred character switch when the editor holds unsaved
/// changes. Returns `true` when the switch must wait for the discard dialog;
/// the caller then skips `select_character`.
pub(crate) fn defer_character_switch_if_unsaved(
    target: usize,
    world: &mut World,
    ui_entity: Entity,
) -> bool {
    if world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.editor_has_unsaved_changes())
    {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            state.0.character_editor_pending_character = Some(target);
        }
        return true;
    }
    false
}

/// Applies a character switch that the discard dialog confirmed, then drops
/// the editor buffers so the page reloads for the newly selected character.
pub(crate) fn confirm_pending_character_switch(
    settings: &mut CharacterSettings,
    emotion_queue: Option<&mut EmotionQueue>,
    now_secs: f64,
    world: &mut World,
    ui_entity: Entity,
) {
    let Some(target) = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.character_editor_pending_character)
    else {
        return;
    };
    push_default_expression(settings.select_character(target), emotion_queue, now_secs);
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.reset_character_editor();
    }
}

/// Reads the card at `path` for editing. A missing file starts from a
/// default card (both load and save support creating a brand-new card);
/// any other read or parse failure is surfaced as an issue and returns
/// `None` so the caller aborts instead of rewriting the card.
fn read_editable_card(
    path: &Path,
    world: &mut World,
    ui_entity: Entity,
) -> Option<CharacterCardV3> {
    let location = card_file_label(path);
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(card) => Some(card),
            Err(error) => {
                set_editor_errors(
                    world,
                    ui_entity,
                    vec![EditorIssue {
                        location,
                        message: i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "character-editor-parse-error",
                            error = error.to_string()
                        ),
                        severity: EditorSeverity::Error,
                    }],
                );
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Some(CharacterCardV3::default())
        }
        Err(error) => {
            set_editor_errors(
                world,
                ui_entity,
                vec![EditorIssue {
                    location,
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-read-error",
                        error = error.to_string()
                    ),
                    severity: EditorSeverity::Error,
                }],
            );
            None
        }
    }
}

/// Applies every editor buffer to `card`. Fields the editor does not expose
/// (assets, expressions, unknown `data` keys) come from the on-disk card and
/// are preserved; `None` structured buffers leave the matching card section
/// untouched.
fn build_card_from_buffers(snapshot: &UiState, mut card: CharacterCardV3) -> CharacterCardV3 {
    card.data.name.clone_from(&snapshot.character_editor_name);
    card.data
        .description
        .clone_from(&snapshot.character_editor_description);
    card.data
        .personality
        .clone_from(&snapshot.character_editor_personality);
    card.data
        .scenario
        .clone_from(&snapshot.character_editor_scenario);
    card.data
        .system_prompt
        .clone_from(&snapshot.character_editor_system_prompt);
    card.data
        .mes_example
        .clone_from(&snapshot.character_editor_mes_example);
    card.data
        .first_mes
        .clone_from(&snapshot.character_editor_first_mes);
    card.data
        .creator_notes
        .clone_from(&snapshot.character_editor_creator_notes);
    card.data
        .post_history_instructions
        .clone_from(&snapshot.character_editor_post_history);
    card.data
        .alternate_greetings
        .clone_from(&snapshot.character_editor_alternate_greetings);
    if let Some(book) = &snapshot.character_editor_lorebook {
        card.data.character_book = Some(book.clone());
    }
    if let Some(catalog) = &snapshot.character_editor_motion_catalog {
        let mut ene = card.data.extensions.ene.clone().unwrap_or_default();
        ene.motion_catalog = if catalog.motions.is_empty() && catalog.idle_lower.is_none() {
            None
        } else {
            Some(catalog.clone())
        };
        card.data.extensions.ene = Some(ene);
    }
    card
}

/// Semantic + asset validation of the assembled card. Every finding carries
/// the field path it refers to; `Error` findings block saving, `Warning`
/// findings are informational.
fn validate_card(card: &CharacterCardV3, card_dir: &Path, assets_dir: &Path) -> Vec<EditorIssue> {
    let mut issues = Vec::new();
    if card.data.name.trim().is_empty() {
        issues.push(EditorIssue {
            location: "data.name".to_string(),
            message: i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-name-required"),
            severity: EditorSeverity::Error,
        });
    }
    if card.data.first_mes.trim().is_empty() {
        issues.push(EditorIssue {
            location: "data.first_mes".to_string(),
            message: i18n_embed_fl::fl!(crate::i18n::loader(), "character-editor-first-mes-empty"),
            severity: EditorSeverity::Warning,
        });
    }
    for (index, greeting) in card.data.alternate_greetings.iter().enumerate() {
        if greeting.trim().is_empty() {
            let human_index = index + 1;
            issues.push(EditorIssue {
                location: format!("data.alternate_greetings[{index}]"),
                message: i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-greeting-empty",
                    index = human_index
                ),
                severity: EditorSeverity::Error,
            });
        }
    }
    if let Some(book) = &card.data.character_book {
        for (index, entry) in book.entries.iter().enumerate() {
            let human_index = index + 1;
            let location = format!("data.character_book.entries[{index}]");
            let has_trigger = entry.keys.iter().any(|key| !key.trim().is_empty());
            if !has_trigger && !entry.constant.unwrap_or(false) {
                issues.push(EditorIssue {
                    location: format!("{location}.keys"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-keys-required",
                        index = human_index
                    ),
                    severity: EditorSeverity::Error,
                });
            }
            if entry.content.trim().is_empty() {
                issues.push(EditorIssue {
                    location: format!("{location}.content"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-content-required",
                        index = human_index
                    ),
                    severity: EditorSeverity::Error,
                });
            }
            if entry.selective == Some(true)
                && entry.secondary_keys.as_ref().is_none_or(Vec::is_empty)
            {
                issues.push(EditorIssue {
                    location: format!("{location}.secondary_keys"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-selective-keys-required",
                        index = human_index
                    ),
                    severity: EditorSeverity::Warning,
                });
            }
            if entry.use_regex
                && let Some(bad_key) = entry
                    .keys
                    .iter()
                    .find(|key| regex::Regex::new(key).is_err())
            {
                issues.push(EditorIssue {
                    location: format!("{location}.keys"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-lorebook-regex-invalid",
                        index = human_index,
                        key = bad_key.clone()
                    ),
                    severity: EditorSeverity::Warning,
                });
            }
        }
    }
    if let Some(catalog) = card
        .data
        .extensions
        .ene
        .as_ref()
        .and_then(|ene| ene.motion_catalog.as_ref())
    {
        for (index, motion) in catalog.motions.iter().enumerate() {
            let human_index = index + 1;
            let location = format!("extensions.ene.motion_catalog.motions[{index}]");
            if motion.name.trim().is_empty() {
                issues.push(EditorIssue {
                    location: format!("{location}.name"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-motion-name-required",
                        index = human_index
                    ),
                    severity: EditorSeverity::Error,
                });
            }
            if motion.path.trim().is_empty() {
                issues.push(EditorIssue {
                    location: format!("{location}.path"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-motion-path-required",
                        index = human_index
                    ),
                    severity: EditorSeverity::Error,
                });
            } else if escapes_card_dir(&motion.path) {
                issues.push(EditorIssue {
                    location: format!("{location}.path"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-motion-path-unsafe",
                        index = human_index
                    ),
                    severity: EditorSeverity::Error,
                });
            } else if !is_regular_file(&card_dir.join(&motion.path)) {
                issues.push(EditorIssue {
                    location: format!("{location}.path"),
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-motion-file-missing",
                        index = human_index,
                        path = motion.path.clone()
                    ),
                    severity: EditorSeverity::Warning,
                });
            }
        }
        if let Some(idle) = &catalog.idle_lower
            && !catalog.motions.iter().any(|motion| &motion.name == idle)
        {
            issues.push(EditorIssue {
                location: "extensions.ene.motion_catalog.idle_lower".to_string(),
                message: i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "character-editor-motion-idle-unknown",
                    name = idle
                ),
                severity: EditorSeverity::Warning,
            });
        }
    }
    for (index, asset) in card.data.assets.iter().enumerate() {
        if asset.ene_kind().is_none() {
            continue;
        }
        let location = format!("data.assets[{index}].uri");
        match resolve_asset_uri(&asset.uri) {
            Ok(ResolvedAssetUri::Embedded(path)) => {
                if !is_regular_file(&card_dir.join(&path)) {
                    let severity = match asset.ene_kind() {
                        Some(EneAssetKind::Vrm) => EditorSeverity::Error,
                        _ => EditorSeverity::Warning,
                    };
                    issues.push(EditorIssue {
                        location,
                        message: i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "character-editor-asset-file-missing",
                            path = path.display().to_string()
                        ),
                        severity,
                    });
                }
            }
            Ok(ResolvedAssetUri::AppDefault) => {
                let default = match asset.ene_kind() {
                    Some(EneAssetKind::Vrm) => DEFAULT_VRM_PATH,
                    _ => DEFAULT_VRMA_PATH,
                };
                if !is_regular_file(&assets_dir.join(default)) {
                    let severity = match asset.ene_kind() {
                        Some(EneAssetKind::Vrm) => EditorSeverity::Error,
                        _ => EditorSeverity::Warning,
                    };
                    issues.push(EditorIssue {
                        location,
                        message: i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "character-editor-asset-default-missing"
                        ),
                        severity,
                    });
                }
            }
            Ok(ResolvedAssetUri::Remote(_)) => {
                issues.push(EditorIssue {
                    location,
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-asset-remote-unverified"
                    ),
                    severity: EditorSeverity::Warning,
                });
            }
            Ok(ResolvedAssetUri::Data { .. }) => {
                issues.push(EditorIssue {
                    location,
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-asset-data-unverified"
                    ),
                    severity: EditorSeverity::Warning,
                });
            }
            Err(error) => {
                issues.push(EditorIssue {
                    location,
                    message: i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "character-editor-asset-uri-invalid",
                        error = error.to_string()
                    ),
                    severity: EditorSeverity::Error,
                });
            }
        }
    }
    issues
}

/// Copies the pre-save card to `<name>.bak` once. The first backup is never
/// overwritten, so after repeated saves it still holds the pre-edit original
/// and a buggy rewrite stays recoverable.
fn backup_card(path: &Path) -> Result<(), std::io::Error> {
    if !path.exists() {
        return Ok(());
    }
    let mut backup_name = path.as_os_str().to_owned();
    backup_name.push(".bak");
    let backup = PathBuf::from(backup_name);
    if backup.exists() {
        return Ok(());
    }
    std::fs::copy(path, backup).map(|_| ())
}

/// `character.{code}.json` sidecar files next to the card. The editor edits
/// the base card only, so these are surfaced as a notice rather than edited.
fn sidecar_diffs(card_path: &Path) -> Vec<String> {
    let Some(dir) = card_path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let code = name
                .strip_prefix("character.")
                .and_then(|rest| rest.strip_suffix(".json"))?;
            if code.is_empty() {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

/// Whether a motion path can escape the character folder: absolute paths and
/// `..` traversal components are both rejected.
fn escapes_card_dir(path: &str) -> bool {
    Path::new(path).is_absolute() || path.split(['/', '\\']).any(|component| component == "..")
}

/// `true` when the path names a regular file without going through symlinks.
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// The card file's own name, used as the issue location for file-level errors.
fn card_file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
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
    use crate::settings::{CharacterEntry, CharacterState};
    use parking_lot::RwLock;

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

    fn editor_errors(world: &World, entity: Entity) -> Vec<EditorIssue> {
        world
            .get::<UiStateComponent>(entity)
            .map(|s| s.0.character_editor_validation_errors.clone())
            .unwrap_or_default()
    }

    fn save(world: &mut World, entity: Entity, path: &Path, assets_dir: &Path) {
        save_character_card(
            &path.to_string_lossy(),
            &assets_dir.to_string_lossy(),
            world,
            entity,
        );
    }

    fn validate(world: &mut World, entity: Entity, path: &Path, assets_dir: &Path) {
        validate_character_card(
            &path.to_string_lossy(),
            &assets_dir.to_string_lossy(),
            world,
            entity,
        );
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

    fn test_settings(names: &[&str]) -> CharacterSettings {
        CharacterSettings {
            assets_dir: PathBuf::from("/tmp/ene-test-assets"),
            characters: names
                .iter()
                .map(|name| CharacterEntry {
                    name: (*name).to_string(),
                    folder: (*name).to_string(),
                    vrm_paths: Vec::new(),
                    motion_paths: Vec::new(),
                    motion_names: Vec::new(),
                    card_path: format!("characters/{name}/character.json"),
                    default_motion: None,
                })
                .collect(),
            character_state: CharacterState {
                selected_character: 0,
                ..Default::default()
            },
            store: Arc::new(RwLock::new(ene_config::ConfigStore::from_config(
                ene_config::EneConfig::default(),
            ))),
        }
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

        save(&mut world, entity, &path, tmp.path());

        let errors = editor_errors(&world, entity);
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("Failed to parse card")),
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

        save(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity != EditorSeverity::Error),
            "a new default card may warn (empty greeting) but must save, got {issues:?}"
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

        save(&mut world, entity, &dir_path, tmp.path());

        let errors = editor_errors(&world, entity);
        assert!(
            errors
                .iter()
                .any(|issue| issue.message.contains("Failed to read card")),
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
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_first_mes = "hello".to_string();
        }

        save(&mut world, entity, &path, tmp.path());

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

    /// Validation must report the exact field path for every finding so the
    /// UI can point the user at the broken section.
    #[test]
    fn validate_reports_error_locations() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Old",
                    "alternate_greetings": ["fine"],
                    "character_book": {
                        "entries": [{
                            "keys": ["lab"],
                            "content": "cold",
                            "enabled": true,
                            "insertion_order": 0,
                            "use_regex": false
                        }]
                    }
                }
            }"#,
        )
        .expect("seed card");
        let (mut world, entity) = editor_world("");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_alternate_greetings = vec![String::new()];
            let mut book = ene_config::Lorebook::default();
            book.entries.push(ene_config::LorebookEntry {
                keys: Vec::new(),
                content: String::new(),
                enabled: true,
                insertion_order: 0,
                use_regex: false,
                constant: Some(false),
                case_sensitive: None,
                name: None,
                priority: None,
                id: None,
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
                extra: indexmap::IndexMap::new(),
                extensions: std::collections::HashMap::new(),
            });
            state.0.character_editor_lorebook = Some(book);
        }

        validate(&mut world, entity, &path, tmp.path());

        let locations = editor_errors(&world, entity)
            .into_iter()
            .map(|issue| issue.location)
            .collect::<Vec<_>>();
        for expected in [
            "data.name",
            "data.alternate_greetings[0]",
            "data.character_book.entries[0].keys",
            "data.character_book.entries[0].content",
        ] {
            assert!(
                locations.iter().any(|location| location == expected),
                "expected {expected:?} among {locations:?}"
            );
        }
    }

    /// A declared VRM whose file is missing must block the save — that is the
    /// broken-startup-path case the editor exists to prevent.
    #[test]
    fn save_blocks_on_missing_declared_vrm() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Old",
                    "assets": [{
                        "type": "x_vrm",
                        "uri": "embeded://model.vrm",
                        "name": "Model",
                        "ext": "vrm"
                    }]
                }
            }"#,
        )
        .expect("seed card");
        let before = std::fs::read_to_string(&path).expect("read original");
        let (mut world, entity) = editor_world("Edited");

        save(&mut world, entity, &path, tmp.path());

        let errors = editor_errors(&world, entity);
        assert!(
            errors
                .iter()
                .any(|issue| issue.location == "data.assets[0].uri"
                    && issue.severity == EditorSeverity::Error),
            "missing VRM must block save, got {errors:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            before,
            "a blocked save must leave the card untouched"
        );
    }

    /// A remote asset URL cannot be verified locally; saving is allowed but a
    /// warning stays visible.
    #[test]
    fn save_allows_remote_asset_with_warning() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Old",
                    "assets": [{
                        "type": "x_vrm",
                        "uri": "https://example.com/model.vrm",
                        "name": "Model",
                        "ext": "vrm"
                    }]
                }
            }"#,
        )
        .expect("seed card");
        let (mut world, entity) = editor_world("Edited");

        save(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues
                .iter()
                .any(|issue| issue.location == "data.assets[0].uri"
                    && issue.severity == EditorSeverity::Warning),
            "remote asset should surface a warning, got {issues:?}"
        );
        assert!(
            !world
                .get::<UiStateComponent>(entity)
                .unwrap()
                .0
                .character_editor_modified,
            "a successful save clears the modified flag"
        );
    }

    /// The pre-save card is backed up once; repeated saves keep the original.
    #[test]
    fn save_backs_up_original_once() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(&path, seed_card_json()).expect("seed card");
        let (mut world, entity) = editor_world("Edited");

        save(&mut world, entity, &path, tmp.path());
        let backup = tmp.path().join("character.json.bak");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup exists"),
            seed_card_json(),
            "the original card must be preserved next to the saved one"
        );

        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_name = "Second Edit".to_string();
        }
        save(&mut world, entity, &path, tmp.path());
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup still exists"),
            seed_card_json(),
            "the first backup must never be overwritten"
        );
    }

    /// A motion path with `..` traversal is rejected as unsafe.
    #[test]
    fn validate_rejects_motion_path_traversal() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Ene");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            let mut catalog = ene_config::MotionCatalog::default();
            catalog.motions.push(ene_config::MotionEntry {
                name: "Sneaky".to_string(),
                path: "../evil.vrma".to_string(),
                layer: None,
                extra: indexmap::IndexMap::new(),
            });
            state.0.character_editor_motion_catalog = Some(catalog);
        }

        validate(&mut world, entity, &path, tmp.path());

        assert!(
            editor_errors(&world, entity).iter().any(|issue| {
                issue.location == "extensions.ene.motion_catalog.motions[0].path"
                    && issue.severity == EditorSeverity::Error
            }),
            "traversal motion path must be an error"
        );
    }

    /// Load → edit → save must round-trip lorebook entries and the motion
    /// catalog without dropping fields the editor does not expose.
    #[test]
    fn save_preserves_lorebook_and_motion_catalog() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Old",
                    "first_mes": "hello",
                    "character_book": {
                        "name": "World",
                        "vendorBookKey": "keep",
                        "entries": [{
                            "keys": ["lab"],
                            "content": "cold",
                            "enabled": true,
                            "insertion_order": 0,
                            "use_regex": false,
                            "id": 42,
                            "probability": 0.5,
                            "uid": 12345,
                            "extensions": {"keep": "me"}
                        }]
                    },
                    "extensions": {
                        "ene": {
                            "vendorEneKey": "keep",
                            "motion_catalog": {
                                "idle_lower": "Idle",
                                "motions": [{
                                    "name": "Idle",
                                    "path": "motions/idle.vrma",
                                    "layer": "lower"
                                }]
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("seed card");
        std::fs::create_dir(tmp.path().join("motions")).expect("create motions dir");
        std::fs::write(tmp.path().join("motions/idle.vrma"), "idle").expect("seed idle motion");
        std::fs::write(tmp.path().join("motions/wave.vrma"), "wave").expect("seed wave motion");
        let (mut world, entity) = editor_world("unused");

        load_character_card(&path.to_string_lossy(), &mut world, entity);
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_name = "Edited".to_string();
            state.0.character_editor_lorebook.as_mut().unwrap().entries[0].content =
                "warm".to_string();
            state
                .0
                .character_editor_motion_catalog
                .as_mut()
                .unwrap()
                .motions
                .push(ene_config::MotionEntry {
                    name: "Wave".to_string(),
                    path: "motions/wave.vrma".to_string(),
                    layer: None,
                    extra: indexmap::IndexMap::new(),
                });
        }

        save(&mut world, entity, &path, tmp.path());

        assert!(
            editor_errors(&world, entity).is_empty(),
            "save must succeed silently"
        );
        let on_disk = std::fs::read_to_string(&path).expect("read card");
        let parsed: ene_config::CharacterCardV3 = serde_json::from_str(&on_disk).expect("parse");
        let book = parsed.data.character_book.expect("lorebook kept");
        assert_eq!(book.name.as_deref(), Some("World"));
        assert_eq!(
            book.extra.get("vendorBookKey"),
            Some(&serde_json::json!("keep")),
            "unknown lorebook-level keys must survive the save"
        );
        let entry = &book.entries[0];
        assert_eq!(entry.content, "warm", "edited content must be applied");
        assert_eq!(
            entry.id,
            Some(serde_json::json!(42)),
            "unexposed id must survive"
        );
        assert_eq!(
            entry.extra.get("probability"),
            Some(&serde_json::json!(0.5)),
            "vendor entry keys must survive the save"
        );
        assert_eq!(
            entry.extra.get("uid"),
            Some(&serde_json::json!(12345)),
            "vendor entry keys must survive the save"
        );
        assert_eq!(
            entry.extensions.get("keep"),
            Some(&serde_json::json!("me")),
            "unexposed entry extensions must survive"
        );
        let ene = parsed.data.extensions.ene.expect("ene block kept");
        let catalog = ene.motion_catalog.as_ref().expect("motion catalog kept");
        assert_eq!(catalog.motions.len(), 2);
        assert_eq!(
            catalog.motions[0].layer,
            Some(ene_config::MotionLayer::Lower),
            "unexposed layer must survive"
        );
        assert_eq!(catalog.motions[1].name, "Wave");
        assert_eq!(
            ene.extra.get("vendorEneKey"),
            Some(&serde_json::json!("keep")),
            "unknown extensions.ene keys must survive the save"
        );
    }

    /// The load path records locale sidecars so the page can warn that the
    /// editor only touches the base-language card.
    #[test]
    fn load_records_locale_sidecars() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(&path, seed_card_json()).expect("seed card");
        std::fs::write(
            tmp.path().join("character.ja.json"),
            r#"{"description": "日本語の説明"}"#,
        )
        .expect("seed sidecar");
        let (mut world, entity) = editor_world("unused");

        load_character_card(&path.to_string_lossy(), &mut world, entity);

        assert_eq!(
            world
                .get::<UiStateComponent>(entity)
                .unwrap()
                .0
                .character_editor_locale_diffs,
            ["character.ja.json"]
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

    /// Unsaved edits defer the switch to the discard dialog; confirming
    /// applies the selection and drops the buffers so the page reloads.
    #[test]
    fn character_switch_defers_until_discard_confirmed() {
        let mut settings = test_settings(&["Alicia", "Blanc"]);
        let (mut world, entity) = editor_world("Edited");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_loaded = true;
            state.0.character_editor_modified = true;
        }

        assert!(
            defer_character_switch_if_unsaved(1, &mut world, entity),
            "unsaved edits must defer the switch"
        );
        assert_eq!(
            settings.character_state.selected_character, 0,
            "the selection must not move before confirmation"
        );
        {
            let state = world.get::<UiStateComponent>(entity).unwrap();
            assert_eq!(state.0.character_editor_pending_character, Some(1));
            assert!(state.0.editor_dialog_pending());
        }

        confirm_pending_character_switch(&mut settings, None, 0.0, &mut world, entity);

        assert_eq!(settings.character_state.selected_character, 1);
        let state = world.get::<UiStateComponent>(entity).unwrap();
        assert!(!state.0.character_editor_modified);
        assert!(!state.0.character_editor_loaded);
        assert!(state.0.character_editor_pending_character.is_none());
    }

    /// "Keep editing" cancels the deferred switch and keeps both the buffers
    /// and the current selection.
    #[test]
    fn character_switch_cancel_keeps_edits_and_selection() {
        let settings = test_settings(&["Alicia", "Blanc"]);
        let (mut world, entity) = editor_world("Edited");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_loaded = true;
            state.0.character_editor_modified = true;
        }
        defer_character_switch_if_unsaved(1, &mut world, entity);
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.cancel_editor_dialog();
        }

        let state = world.get::<UiStateComponent>(entity).unwrap();
        assert!(state.0.character_editor_modified, "edits must survive");
        assert!(!state.0.editor_dialog_pending());
        assert_eq!(settings.character_state.selected_character, 0);
    }

    #[test]
    fn character_switch_without_edits_switches_immediately() {
        let settings = test_settings(&["Alicia", "Blanc"]);
        let (mut world, entity) = editor_world("Alicia");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            state.0.character_editor_loaded = true;
        }

        assert!(!defer_character_switch_if_unsaved(1, &mut world, entity));
        assert_eq!(settings.character_state.selected_character, 0);
    }

    /// Discarding a close keeps `close_requested` set so the runtime can
    /// complete the hide once the dirty flag is gone.
    #[test]
    fn discard_decision_close_flow_keeps_request_flags() {
        let mut state = UiState {
            character_editor_loaded: true,
            character_editor_modified: true,
            character_editor_close_requested: true,
            ..UiState::default()
        };

        assert!(!apply_discard_decision(&mut state));
        assert!(!state.character_editor_modified);
        assert!(!state.character_editor_loaded);
        assert!(state.character_editor_close_requested);
    }

    /// Discarding a reload resets the buffers so the page reloads from disk.
    #[test]
    fn discard_decision_reload_resets_buffers() {
        let mut state = UiState {
            character_editor_loaded: true,
            character_editor_modified: true,
            character_editor_reload_pending: true,
            character_editor_name: "Edited".to_string(),
            ..UiState::default()
        };

        assert!(!apply_discard_decision(&mut state));
        assert!(!state.character_editor_modified);
        assert!(!state.character_editor_reload_pending);
        assert!(state.character_editor_name.is_empty());
    }

    /// A deferred character switch is reported so the caller can apply it
    /// after releasing the world borrow.
    #[test]
    fn discard_decision_switch_returns_pending_target() {
        let mut state = UiState {
            character_editor_modified: true,
            character_editor_pending_character: Some(2),
            ..UiState::default()
        };

        assert!(apply_discard_decision(&mut state));
        assert_eq!(state.character_editor_pending_character, Some(2));
    }

    /// Absolute motion paths are rejected just like `..` traversal.
    #[test]
    fn validate_rejects_absolute_motion_path() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Ene");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            let mut catalog = ene_config::MotionCatalog::default();
            catalog.motions.push(ene_config::MotionEntry {
                name: "Absolute".to_string(),
                path: "/etc/evil.vrma".to_string(),
                layer: None,
                extra: indexmap::IndexMap::new(),
            });
            state.0.character_editor_motion_catalog = Some(catalog);
        }

        validate(&mut world, entity, &path, tmp.path());

        assert!(
            editor_errors(&world, entity).iter().any(|issue| {
                issue.location == "extensions.ene.motion_catalog.motions[0].path"
                    && issue.severity == EditorSeverity::Error
            }),
            "absolute motion path must be an error"
        );
    }

    /// A missing bundled default VRM blocks saving; the same default VRMA
    /// only warns because the runtime tolerates a missing motion.
    #[test]
    fn validate_default_asset_missing_vrm_error_vrma_warning() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Ene",
                    "first_mes": "hi",
                    "assets": [
                        {"type": "x_vrm", "uri": "ccdefault:", "name": "model", "ext": "vrm"},
                        {"type": "x_vrma", "uri": "ccdefault:", "name": "motion", "ext": "vrma"}
                    ]
                }
            }"#,
        )
        .expect("seed card");
        let (mut world, entity) = editor_world("Ene");

        validate(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.assets[0].uri" && issue.severity == EditorSeverity::Error
            }),
            "missing default VRM must be an error, got {issues:?}"
        );
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.assets[1].uri" && issue.severity == EditorSeverity::Warning
            }),
            "missing default VRMA must be a warning, got {issues:?}"
        );
    }

    /// Unsupported URI schemes are errors; data URLs are unverifiable
    /// warnings.
    #[test]
    fn validate_invalid_uri_error_and_data_url_warning() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Ene",
                    "first_mes": "hi",
                    "assets": [
                        {"type": "x_vrm", "uri": "ftp://host/model.vrm", "name": "bad", "ext": "vrm"},
                        {"type": "x_vrm", "uri": "data:application/octet-stream;base64,AAAA", "name": "embedded", "ext": "vrm"}
                    ]
                }
            }"#,
        )
        .expect("seed card");
        let (mut world, entity) = editor_world("Ene");

        validate(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.assets[0].uri" && issue.severity == EditorSeverity::Error
            }),
            "unsupported scheme must be an error, got {issues:?}"
        );
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.assets[1].uri" && issue.severity == EditorSeverity::Warning
            }),
            "data URL must be an unverifiable warning, got {issues:?}"
        );
    }

    /// A missing declared VRMA warns but does not block the save.
    #[test]
    fn missing_declared_vrma_warns_but_saves() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        std::fs::write(
            &path,
            r#"{
                "spec": "chara_card_v3",
                "spec_version": "3.0",
                "data": {
                    "name": "Old",
                    "first_mes": "hi",
                    "assets": [{
                        "type": "x_vrma",
                        "uri": "embeded://wave.vrma",
                        "name": "Wave",
                        "ext": "vrma"
                    }]
                }
            }"#,
        )
        .expect("seed card");
        let (mut world, entity) = editor_world("Edited");

        save(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.assets[0].uri" && issue.severity == EditorSeverity::Warning
            }),
            "missing VRMA must warn, got {issues:?}"
        );
        assert!(
            !world
                .get::<UiStateComponent>(entity)
                .unwrap()
                .0
                .character_editor_modified,
            "a warning-only save must still write the card"
        );
    }

    /// An empty first message is legal `CCv3`, so it warns instead of blocking.
    #[test]
    fn empty_first_mes_warns_but_saves() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Ene");

        save(&mut world, entity, &path, tmp.path());

        let issues = editor_errors(&world, entity);
        assert!(
            issues.iter().any(|issue| {
                issue.location == "data.first_mes" && issue.severity == EditorSeverity::Warning
            }),
            "empty first message must warn, got {issues:?}"
        );
        assert!(path.exists(), "the save must still succeed");
    }

    /// A selective entry without secondary keys cannot actually be
    /// selective; surface it at the entry's secondary-keys field.
    #[test]
    fn selective_entry_without_secondary_keys_warns() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Ene");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            let mut book = ene_config::Lorebook::default();
            book.entries.push(ene_config::LorebookEntry {
                keys: vec!["lab".to_string()],
                content: "cold".to_string(),
                enabled: true,
                insertion_order: 0,
                use_regex: false,
                constant: Some(false),
                case_sensitive: None,
                name: None,
                priority: None,
                id: None,
                comment: None,
                selective: Some(true),
                secondary_keys: None,
                position: None,
                extensions: std::collections::HashMap::new(),
                extra: indexmap::IndexMap::new(),
            });
            state.0.character_editor_lorebook = Some(book);
        }

        validate(&mut world, entity, &path, tmp.path());

        assert!(
            editor_errors(&world, entity).iter().any(|issue| {
                issue.location == "data.character_book.entries[0].secondary_keys"
                    && issue.severity == EditorSeverity::Warning
            }),
            "selective entry without secondary keys must warn"
        );
    }

    /// A trigger key that does not compile as a regex is pointed at even
    /// though the runtime tolerates it.
    #[test]
    fn invalid_regex_key_warns() {
        let tmp = tempfile::tempdir().expect("OS allows temp directory creation");
        let path = tmp.path().join("character.json");
        let (mut world, entity) = editor_world("Ene");
        {
            let mut state = world.get_mut::<UiStateComponent>(entity).unwrap();
            let mut book = ene_config::Lorebook::default();
            book.entries.push(ene_config::LorebookEntry {
                keys: vec!["(".to_string()],
                content: "cold".to_string(),
                enabled: true,
                insertion_order: 0,
                use_regex: true,
                constant: Some(false),
                case_sensitive: None,
                name: None,
                priority: None,
                id: None,
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
                extensions: std::collections::HashMap::new(),
                extra: indexmap::IndexMap::new(),
            });
            state.0.character_editor_lorebook = Some(book);
        }

        validate(&mut world, entity, &path, tmp.path());

        assert!(
            editor_errors(&world, entity).iter().any(|issue| {
                issue.location == "data.character_book.entries[0].keys"
                    && issue.severity == EditorSeverity::Warning
            }),
            "a trigger key that fails to compile as regex must warn"
        );
    }
}
