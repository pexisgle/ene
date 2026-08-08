use ene_config::EneConfigError;
use ene_config::{ConfigTarget, HasConfigKey};
use indexmap::IndexMap;

/// Motion body layer classification.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(crate = "crate::serde", rename_all = "lowercase")]
#[schemars(crate = "crate::schemars")]
pub enum MotionLayer {
    /// Upper-body gesture + expression.
    Upper,
    /// Lower-body idle loop.
    Lower,
    /// Full-body override (preempts upper/lower).
    Full,
}

impl MotionLayer {
    /// Stable display / log label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Full => "full",
        }
    }

    /// Parse a stable label (the inverse of [`as_str`](Self::as_str)).
    ///
    /// Returns `None` for unknown labels so callers can decide on a
    /// fallback rather than silently coercing to a specific layer.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "upper" => Some(Self::Upper),
            "lower" => Some(Self::Lower),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// A single motion entry with a display name and relative file path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct MotionEntry {
    /// Display name for the motion (e.g. `"VRMA_01"`).
    pub name: String,
    /// Relative path to the motion file (e.g. `"motions/VRMA_01.vrma"`).
    pub path: String,
    /// Body layer this motion targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<MotionLayer>,
    /// Catch-all for vendor motion fields this build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Structured motion catalog for a character.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, Default)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct MotionCatalog {
    /// Name of the idle lower-body animation loop (matches a `MotionEntry.name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_lower: Option<String>,
    /// Registered motions indexed by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub motions: Vec<MotionEntry>,
    /// Catch-all for vendor catalog fields this build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Per-character visual config used by the desktop GUI.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct CharacterConfig {
    /// 3D position of the character model in the scene.
    pub character_position: [f32; 3],
    /// Scale factor applied to the character model.
    pub model_scale: f32,
    /// How strongly the character looks toward the user (0.0–1.0).
    pub look_at_strength: f32,
    /// Name of the default motion (matches a `MotionEntry.name`).
    pub default_motion: String,
    /// Name of the default expression (e.g. `"neutral"`).
    pub default_expression: String,
    /// Language code override for this character's card (e.g. `"ja"`).
    /// Empty (the default) inherits the app language; the value is
    /// canonicalized through [`ene_config::resolve_language_alias`] when the
    /// card is loaded.
    pub language: String,

    /// Catch-all for extra configurations.
    ///
    /// An [`IndexMap`] so the user's section order is preserved across a save;
    /// its order-insensitive `PartialEq` keeps the dirty-tracking equality
    /// guard from tripping on key reordering.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: IndexMap<String, serde_json::Value>,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            character_position: [0.0, 0.0, 0.0],
            model_scale: 1.0,
            look_at_strength: 0.6,
            default_motion: String::new(),
            default_expression: "neutral".to_string(),
            language: String::new(),
            extra: IndexMap::new(),
        }
    }
}

impl CharacterConfig {
    /// Deserialise a sub-section from the `extra` map using the type's associated path.
    ///
    /// Walks the map directly, descending into nested objects one level at a time,
    /// instead of rebuilding the entire `extra` map into a JSON `Value` on every call.
    pub fn get_section<T>(&self) -> Result<T, EneConfigError>
    where
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Character);
        let mut current: Option<&serde_json::Value> = None;
        for (i, key) in T::path().iter().enumerate() {
            if i == 0 {
                match self.extra.get(*key) {
                    Some(v) => current = Some(v),
                    None => return Ok(T::default()),
                }
                continue;
            }
            let Some(cur_val) = current else {
                return Ok(T::default());
            };
            match cur_val.as_object().and_then(|o| o.get(*key)) {
                Some(v) => current = Some(v),
                None => return Ok(T::default()),
            }
        }
        let Some(final_val) = current else {
            return Ok(T::default());
        };
        serde_json::from_value(final_val.clone()).map_err(|e| {
            EneConfigError::GenericConfigError(format!(
                "Failed to deserialize character nested section: {e}"
            ))
        })
    }

    /// Serialise and merge a sub-section into the `extra` map using the type's associated path.
    ///
    /// Serialisation goes through [`section_to_value`](ene_config::config::section_to_value)
    /// to avoid the f32→f64 widening artefact.
    ///
    /// Only the section's *declared* fields are written; unknown sub-keys
    /// already present beneath the section path are preserved.
    ///
    /// Reuses [`merge_section`](ene_config::config::merge_section) and
    /// [`set_nested`](ene_config::config::set_nested) for direct map mutation
    /// instead of rebuilding the entire map from a JSON `Value`.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Character);
        let val = ene_config::config::section_to_value(section)?;
        let path = T::path();
        let merged = ene_config::config::merge_section(
            ene_config::config::read_at_path(&self.extra, path),
            &val,
        );
        ene_config::config::set_nested(&mut self.extra, path, merged)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CharacterConfig;
    use super::MotionLayer;
    use ene_config::{ConfigTarget, HasConfigKey};

    #[test]
    fn motion_layer_label_round_trips() {
        for layer in [MotionLayer::Upper, MotionLayer::Lower, MotionLayer::Full] {
            assert_eq!(MotionLayer::from_label(layer.as_str()), Some(layer));
        }
    }

    #[test]
    fn motion_layer_from_label_rejects_unknown() {
        assert_eq!(MotionLayer::from_label("sideways"), None);
        assert_eq!(MotionLayer::from_label(""), None);
    }

    /// A test-only character section used to exercise `set_section` without
    /// pulling in another workspace crate.
    #[derive(serde::Serialize, serde::Deserialize, Default)]
    struct TestCharacterSection {
        intensity: f32,
    }

    impl HasConfigKey for TestCharacterSection {
        const KEY: &'static str = "motion";
        const TARGET: ConfigTarget = ConfigTarget::Character;
        fn path() -> &'static [&'static str] {
            &["motion"]
        }
    }

    /// Writing a character section must merge its declared fields into the
    /// existing subtree, preserving unknown sibling sub-keys.
    #[test]
    fn set_section_preserves_unknown_subkeys() {
        let mut config = CharacterConfig::default();
        config.extra.insert(
            "motion".to_string(),
            serde_json::json!({ "custom_key": "keep-me", "intensity": 0.25 }),
        );

        config
            .set_section(&TestCharacterSection { intensity: 0.5 })
            .expect("set_section succeeds");

        let motion = config
            .extra
            .get("motion")
            .and_then(serde_json::Value::as_object)
            .expect("motion object present");
        assert_eq!(
            motion.get("intensity"),
            Some(&serde_json::json!(0.5)),
            "declared field must be updated"
        );
        assert_eq!(
            motion.get("custom_key"),
            Some(&serde_json::json!("keep-me")),
            "unknown sibling sub-key must survive the section write"
        );
    }
}
