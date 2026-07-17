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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
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
    pub fn get_section<T>(&self) -> Result<T, EneConfigError>
    where
        T: serde::de::DeserializeOwned + Default + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Character);
        let mut cur = serde_json::Value::Object(
            self.extra
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        for key in T::path() {
            match cur.get(key).cloned() {
                Some(v) => cur = v,
                None => return Ok(T::default()),
            }
        }
        serde_json::from_value(cur).map_err(|e| {
            EneConfigError::GenericConfigError(format!(
                "Failed to deserialize character nested section: {e}"
            ))
        })
    }

    /// Serialise and insert a sub-section into the `extra` map using the type's associated path.
    pub fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError>
    where
        T: serde::Serialize + HasConfigKey,
    {
        debug_assert_eq!(T::TARGET, ConfigTarget::Character);
        let val = serde_json::to_value(section).map_err(|e| {
            EneConfigError::GenericConfigError(format!(
                "Failed to serialize character section: {e}"
            ))
        })?;

        let mut root = serde_json::Value::Object(
            self.extra
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let mut cur = &mut root;
        let path = T::path();
        for (i, &key) in path.iter().enumerate() {
            if i == path.len().saturating_sub(1) {
                if let Some(obj) = cur.as_object_mut() {
                    obj.insert(key.to_string(), val);
                }
                break;
            }
            if !cur.is_object() {
                *cur = serde_json::Value::Object(serde_json::Map::new());
            }
            let Some(obj) = cur.as_object_mut() else {
                return Err(EneConfigError::GenericConfigError(
                    "Internal error: expected JSON object".to_string(),
                ));
            };
            cur = obj
                .entry(key.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        }
        if let serde_json::Value::Object(obj) = root {
            self.extra = obj.into_iter().collect();
        }

        Ok(())
    }
}
