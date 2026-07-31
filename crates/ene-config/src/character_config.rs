use crate::error::EneConfigError;
use crate::{ConfigTarget, HasConfigKey};
use std::collections::BTreeMap;

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
    /// Body layer this motion targets (#130).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<MotionLayer>,
}

/// Structured motion catalog for a character (#130).
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

    /// Catch-all for extra configurations.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for CharacterConfig {
    fn default() -> Self {
        Self {
            character_position: [0.0, 0.0, 0.0],
            model_scale: 1.0,
            look_at_strength: 0.6,
            default_motion: String::new(),
            default_expression: "neutral".to_string(),
            extra: BTreeMap::new(),
        }
    }
}

impl CharacterConfig {
    /// Deserialise a sub-section from the `extra` map using the type's associated path.
    ///
    /// Walks the `BTreeMap` directly, descending into nested objects one level at a time,
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

    /// Serialise and insert a sub-section into the `extra` map using the type's associated path.
    ///
    /// Serialisation goes through [`section_to_value`](crate::config::section_to_value)
    /// to avoid the f32→f64 widening artefact (#329).
    ///
    /// Reuses [`set_nested`](crate::config::set_nested) for direct `BTreeMap` mutation
    /// instead of rebuilding the entire map from a JSON `Value`.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Character);
        let val = crate::config::section_to_value(section)?;
        crate::config::set_nested(&mut self.extra, T::path(), val)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MotionLayer;

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
}
