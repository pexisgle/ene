use std::collections::{BTreeMap, HashMap};

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
        pub model_scale: f32 = 1.0,
        pub character_x: f32 = 0.78,
        pub character_y: f32 = 0.5,
        #[serde(default)]
        pub character_positions: HashMap<String, [f32; 2]> = HashMap::new(),
        pub look_at_strength: f32 = 0.6,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_empty_positions() {
        let settings = DesktopSettings::default();
        assert!(settings.character_positions.is_empty());
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
        let json = serde_json::to_string(&settings).unwrap_or_default();
        let parsed: DesktopSettings = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(
            parsed.character_positions.get("soul-a"),
            Some(&[0.42, 0.58]),
        );
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
}
