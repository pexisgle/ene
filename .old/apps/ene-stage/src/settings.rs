use std::collections::{BTreeMap, HashMap, HashSet};

use ene_config::{EneConfigError, load_full_config, save_full_config};

ene_config::define_config!(
    settings,
    "desktop",
    /// Stage client UI, overlay, and core lifetime preferences.
    pub struct DesktopSettings {
        pub core_lifetime: String = "app".to_owned(),
        pub theme: String = "system".to_owned(),
        pub language: String = String::new(),
        pub mic_device: String = String::new(),
        pub spotlight_enabled: bool = true,
        pub caption_enabled: bool = true,
        pub caption_font_size: f32 = 18.0,
        pub caption_position: String = "bottom".to_owned(),
        pub caption_pinned: bool = false,
        pub graphics_quality: String = "high".to_owned(),
        pub always_on_top: bool = true,
        pub transparent_overlay: bool = true,
        pub overlay_monitor_mode: String = "primary".to_owned(),
        pub overlay_monitor_id: String = String::new(),
        pub overlay_monitor_name: String = String::new(),
        pub overlay_monitor_position: [i32; 2] = [0, 0],
        pub overlay_monitor_size: [u32; 2] = [0, 0],
        pub overlay_monitor_scale_factor: f64 = 1.0,
        pub model_scale: f32 = 1.0,
        pub character_x: f32 = 0.78,
        pub character_y: f32 = 0.5,
        #[serde(default)]
        pub character_positions: HashMap<String, [f32; 2]> = HashMap::new(),
        #[serde(default)]
        pub character_scales: HashMap<String, f32> = HashMap::new(),
        pub displayed_soul_ids: Vec<String> = Vec::new(),
        #[serde(default)]
        pub displayed_souls_initialized: bool = false,
        pub look_at_strength: f32 = 0.6,
        pub direct_reactions_enabled: bool = true,
        pub direct_reaction_strength: f32 = 0.7,
        pub direct_reaction_agent: bool = false,
        pub direct_reaction_selects_active: bool = false,
        pub overlay_click_through: bool = true,
        pub onboarding_dismissed: bool = false,
    }
);

#[must_use]
pub fn load_desktop_settings() -> DesktopSettings {
    match load_full_config() {
        Ok(config) => config
            .get_section::<DesktopSettings>()
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "desktop settings section missing; using defaults");
                DesktopSettings::default()
            }),
        Err(err) => {
            tracing::warn!(error = %err, "failed to load config; using desktop defaults");
            DesktopSettings::default()
        }
    }
}

pub fn save_desktop_settings(settings: &DesktopSettings) -> Result<(), EneConfigError> {
    let mut config = load_full_config()?;
    config.set_section(settings)?;
    save_full_config(&config)
}

/// Default normalized position for a body without a saved slot.
pub const DEFAULT_SECONDARY_POSITION: [f32; 2] = [0.22, 0.5];
/// Default normalized position for the active body.
pub const DEFAULT_PRIMARY_POSITION: [f32; 2] = [0.78, 0.5];
/// Default center position used by the per-body recovery action.
pub const CENTER_POSITION: [f32; 2] = [0.5, 0.5];
/// Lower bound shared by the per-body and legacy scale controls.
pub const MODEL_SCALE_MIN: f32 = 0.3;
/// Upper bound shared by the per-body and legacy scale controls.
pub const MODEL_SCALE_MAX: f32 = 2.0;
/// Fallback scale for a missing or malformed per-body value.
pub const DEFAULT_MODEL_SCALE: f32 = 1.0;
/// Conservative scale used when two bodies are arranged side by side.
pub const TWO_BODY_FIT_SCALE: f32 = 0.65;

#[must_use]
pub fn clamp_model_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MODEL_SCALE_MIN, MODEL_SCALE_MAX)
    } else {
        DEFAULT_MODEL_SCALE
    }
}

#[must_use]
pub fn effective_model_scale(settings: &DesktopSettings, soul_id: &str) -> f32 {
    settings.character_scales.get(soul_id).copied().map_or_else(
        || clamp_model_scale(settings.model_scale),
        clamp_model_scale,
    )
}

#[must_use]
pub fn default_position_for(soul_id: &str, active_soul_id: &str) -> [f32; 2] {
    if soul_id == active_soul_id {
        DEFAULT_PRIMARY_POSITION
    } else {
        DEFAULT_SECONDARY_POSITION
    }
}

#[must_use]
pub fn arranged_positions(soul_ids: &[String]) -> Vec<[f32; 2]> {
    match soul_ids.len() {
        0 => Vec::new(),
        1 => vec![CENTER_POSITION],
        _ => soul_ids
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if index.is_multiple_of(2) {
                    [0.3, 0.5]
                } else {
                    [0.7, 0.5]
                }
            })
            .collect(),
    }
}

/// Ensures every loaded soul has an entry in `character_positions`.
///
/// The active soul inherits the legacy scalar keys; other bodies start on
/// the left side so two freshly-seeded bodies do not overlap.
pub fn seed_character_positions(
    positions: &mut BTreeMap<String, [f32; 2]>,
    soul_ids: &[String],
    active_soul_id: &str,
    legacy_active_pos: [f32; 2],
) {
    for soul_id in soul_ids {
        if positions.contains_key(soul_id) {
            continue;
        }
        let pos = if soul_id == active_soul_id {
            legacy_active_pos
        } else {
            DEFAULT_SECONDARY_POSITION
        };
        positions.insert(soul_id.clone(), pos);
    }
}

/// Mirrors the active soul's per-body position into the legacy scalar fields
/// so hand-edited config files stay readable. Reads still prefer the map.
pub fn mirror_active_position(settings: &mut DesktopSettings, active_soul_id: &str) {
    if let Some(pos) = settings.character_positions.get(active_soul_id) {
        settings.character_x = pos[0];
        settings.character_y = pos[1];
    }
}

pub(crate) fn normalize_displayed_souls(
    displayed_soul_ids: &mut Vec<String>,
    initialized: &mut bool,
    available_soul_ids: &[String],
) -> bool {
    let before_ids = displayed_soul_ids.clone();
    let before_initialized = *initialized;
    let mut normalized = Vec::new();
    for soul_id in displayed_soul_ids.iter() {
        if available_soul_ids
            .iter()
            .any(|available| available == soul_id)
            && !normalized.iter().any(|existing| existing == soul_id)
        {
            normalized.push(soul_id.clone());
        }
    }
    if !*initialized && let Some(first) = available_soul_ids.first() {
        if normalized.is_empty() {
            normalized.push(first.clone());
        }
        *initialized = true;
    }
    *displayed_soul_ids = normalized;
    before_ids != *displayed_soul_ids || before_initialized != *initialized
}

pub(crate) fn ordered_visible_souls(
    displayed_soul_ids: &[String],
    temporarily_hidden: &HashSet<String>,
    capacity: usize,
) -> Vec<String> {
    displayed_soul_ids
        .iter()
        .filter(|soul_id| !temporarily_hidden.contains(*soul_id))
        .take(capacity)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_empty_positions() {
        let settings = DesktopSettings::default();
        assert!(settings.character_positions.is_empty());
        assert_eq!(settings.overlay_monitor_mode, "primary");
        assert!(settings.overlay_monitor_id.is_empty());
        assert!(settings.direct_reactions_enabled);
        assert!((settings.direct_reaction_strength - 0.7).abs() < f32::EPSILON);
        assert!(!settings.direct_reaction_agent);
        assert!(!settings.direct_reaction_selects_active);
        assert!(settings.character_scales.is_empty());
        assert!(settings.displayed_soul_ids.is_empty());
        assert!(!settings.displayed_souls_initialized);
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn seeding_gives_active_legacy_and_others_default() {
        let mut positions = BTreeMap::new();
        let souls = ids(&["soul-a", "soul-b"]);
        seed_character_positions(&mut positions, &souls, "soul-a", [0.8, 0.4]);
        let [a_x, a_y] = positions["soul-a"];
        assert!((a_x - 0.8).abs() < f32::EPSILON);
        assert!((a_y - 0.4).abs() < f32::EPSILON);
        let [b_x, b_y] = positions["soul-b"];
        assert!((b_x - DEFAULT_SECONDARY_POSITION[0]).abs() < f32::EPSILON);
        assert!((b_y - DEFAULT_SECONDARY_POSITION[1]).abs() < f32::EPSILON);
    }

    #[test]
    fn seeding_keeps_existing_entries_untouched() {
        let mut positions = BTreeMap::from([("soul-a".to_owned(), [0.3, 0.6])]);
        let souls = ids(&["soul-a", "soul-b"]);
        seed_character_positions(&mut positions, &souls, "soul-a", [0.9, 0.9]);
        let [a_x, a_y] = positions["soul-a"];
        assert!((a_x - 0.3).abs() < f32::EPSILON);
        assert!((a_y - 0.6).abs() < f32::EPSILON);
        let [b_x, b_y] = positions["soul-b"];
        assert!((b_x - DEFAULT_SECONDARY_POSITION[0]).abs() < f32::EPSILON);
        assert!((b_y - DEFAULT_SECONDARY_POSITION[1]).abs() < f32::EPSILON);
    }

    #[test]
    fn seed_prefers_preloaded_map_over_legacy_scalar() {
        let mut positions = BTreeMap::from([("active".to_owned(), [0.11, 0.22])]);
        let souls = ids(&["active"]);
        seed_character_positions(&mut positions, &souls, "active", [0.8, 0.4]);
        let [x, y] = positions["active"];
        assert!((x - 0.11).abs() < f32::EPSILON);
        assert!((y - 0.22).abs() < f32::EPSILON);
    }

    #[test]
    fn settings_roundtrip_preserves_positions_map() {
        let mut settings = DesktopSettings::default();
        settings
            .character_positions
            .insert("soul-a".to_owned(), [0.42, 0.58]);
        settings.displayed_soul_ids = ids(&["soul-a", "soul-b"]);
        settings.displayed_souls_initialized = true;
        let json = serde_json::to_string(&settings).unwrap_or_default();
        let parsed: DesktopSettings = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(
            parsed.character_positions.get("soul-a"),
            Some(&[0.42, 0.58]),
        );
        assert_eq!(parsed.displayed_soul_ids, ids(&["soul-a", "soul-b"]));
        assert!(parsed.displayed_souls_initialized);
    }

    #[test]
    fn settings_roundtrip_preserves_monitor_selection_metadata() {
        let settings = DesktopSettings {
            overlay_monitor_mode: "selected".to_owned(),
            overlay_monitor_id: "name:secondary".to_owned(),
            overlay_monitor_name: "\\\\.\\DISPLAY2".to_owned(),
            overlay_monitor_position: [1920, 0],
            overlay_monitor_size: [2560, 1440],
            overlay_monitor_scale_factor: 1.25,
            ..DesktopSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap_or_default();
        let parsed: DesktopSettings = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(parsed.overlay_monitor_mode, "selected");
        assert_eq!(parsed.overlay_monitor_id, "name:secondary");
        assert_eq!(parsed.overlay_monitor_position, [1920, 0]);
        assert_eq!(parsed.overlay_monitor_size, [2560, 1440]);
        assert!((parsed.overlay_monitor_scale_factor - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_positions_key_deserializes_to_empty_map() {
        // Legacy config files have no character_positions key at all.
        let json = r#"{"core_lifetime":"app","character_x":0.7,"character_y":0.5}"#;
        let parsed: DesktopSettings = serde_json::from_str(json).unwrap_or_default();
        assert!(parsed.character_positions.is_empty());
        assert!((parsed.character_x - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn mirroring_copies_active_soul_into_legacy_scalars() {
        let mut settings = DesktopSettings {
            character_x: 0.1,
            character_y: 0.2,
            ..DesktopSettings::default()
        };
        settings
            .character_positions
            .insert("active".to_owned(), [0.65, 0.35]);
        settings
            .character_positions
            .insert("other".to_owned(), [0.11, 0.22]);
        mirror_active_position(&mut settings, "active");
        assert!((settings.character_x - 0.65).abs() < f32::EPSILON);
        assert!((settings.character_y - 0.35).abs() < f32::EPSILON);
    }

    #[test]
    fn mirroring_without_entry_leaves_scalars_alone() {
        let mut settings = DesktopSettings::default();
        mirror_active_position(&mut settings, "unknown");
        assert!((settings.character_x - 0.78).abs() < f32::EPSILON);
        assert!((settings.character_y - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn effective_scale_prefers_a_valid_per_body_value() {
        let mut settings = DesktopSettings {
            model_scale: 1.4,
            ..DesktopSettings::default()
        };
        settings.character_scales.insert("active".to_owned(), 0.65);

        assert!((effective_model_scale(&settings, "active") - 0.65).abs() < f32::EPSILON);
        assert!((effective_model_scale(&settings, "other") - 1.4).abs() < f32::EPSILON);
    }

    #[test]
    fn malformed_scale_values_fall_back_inside_the_supported_range() {
        assert!((clamp_model_scale(f32::NAN) - DEFAULT_MODEL_SCALE).abs() < f32::EPSILON);
        assert!((clamp_model_scale(-1.0) - MODEL_SCALE_MIN).abs() < f32::EPSILON);
        assert!((clamp_model_scale(9.0) - MODEL_SCALE_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn arranged_positions_keep_one_body_centered_and_two_separated() {
        assert_eq!(arranged_positions(&[]), Vec::<[f32; 2]>::new());
        assert_eq!(arranged_positions(&["a".to_owned()]), vec![CENTER_POSITION]);
        assert_eq!(
            arranged_positions(&["a".to_owned(), "b".to_owned()]),
            vec![[0.3, 0.5], [0.7, 0.5]]
        );
    }

    #[test]
    fn first_available_soul_becomes_the_explained_initial_display() {
        let mut displayed = Vec::new();
        let mut initialized = false;
        let available = ids(&["first", "second"]);

        assert!(normalize_displayed_souls(
            &mut displayed,
            &mut initialized,
            &available
        ));
        assert_eq!(displayed, ids(&["first"]));
        assert!(initialized);
    }

    #[test]
    fn normalization_keeps_user_order_and_drops_stale_duplicates() {
        let mut displayed = ids(&["second", "missing", "first", "second"]);
        let mut initialized = true;
        let available = ids(&["first", "second"]);

        assert!(normalize_displayed_souls(
            &mut displayed,
            &mut initialized,
            &available
        ));
        assert_eq!(displayed, ids(&["second", "first"]));
        assert!(initialized);
    }

    #[test]
    fn explicit_empty_display_stays_empty_after_initialization() {
        let mut displayed = Vec::new();
        let mut initialized = true;
        let available = ids(&["first"]);

        assert!(!normalize_displayed_souls(
            &mut displayed,
            &mut initialized,
            &available
        ));
        assert!(displayed.is_empty());
    }

    #[test]
    fn temporary_hidden_souls_do_not_change_persistent_order() {
        let displayed = ids(&["first", "second", "third"]);
        let temporarily_hidden = HashSet::from(["first".to_owned()]);

        assert_eq!(
            ordered_visible_souls(&displayed, &temporarily_hidden, 2),
            ids(&["second", "third"])
        );
        assert_eq!(displayed, ids(&["first", "second", "third"]));
    }
}
