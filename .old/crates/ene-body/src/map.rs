use crate::config::FallbackSettings;
use crate::error::BodyError;
use crate::queue::{EmotionCue, PerformanceCommand};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Body-owned expression catalog and emotion→expression map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyCatalog {
    pub kind: BodyKind,
    pub expressions: Vec<String>,
    pub motions: Vec<String>,
    pub emotion_map: BTreeMap<String, MappedExpression>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    Text,
    Vrm,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MappedExpression {
    pub expression: String,
    #[serde(default = "one")]
    pub intensity_scale: f32,
}

fn one() -> f32 {
    1.0
}

impl MappedExpression {
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.intensity_scale.clamp(0.2, 2.0)
    }
}

impl Default for BodyCatalog {
    fn default() -> Self {
        Self::text_default()
    }
}

impl BodyCatalog {
    #[must_use]
    pub fn text_default() -> Self {
        let expressions = vec![
            "happy".into(),
            "joyful".into(),
            "calm".into(),
            "sad".into(),
            "angry".into(),
            "surprised".into(),
            "thinking".into(),
        ];
        let mut emotion_map = BTreeMap::new();
        for label in [
            "happy",
            "joyful",
            "excited",
            "amused",
            "content",
            "calm",
            "relaxed",
            "sleepy",
            "bored",
            "curious",
            "interested",
            "surprised",
            "confused",
            "worried",
            "anxious",
            "sad",
            "lonely",
            "disappointed",
            "embarrassed",
            "shy",
            "angry",
            "annoyed",
            "jealous",
            "determined",
        ] {
            let expression = match label {
                "excited" | "amused" | "joyful" => "joyful",
                "content" | "relaxed" | "sleepy" | "bored" => "calm",
                "curious" | "interested" | "confused" | "worried" | "anxious" | "determined" => {
                    "thinking"
                }
                "lonely" | "disappointed" | "embarrassed" | "shy" => "sad",
                "annoyed" | "jealous" => "angry",
                other => other,
            };
            emotion_map.insert(
                label.to_owned(),
                MappedExpression {
                    expression: expression.to_owned(),
                    intensity_scale: 1.0,
                },
            );
        }
        Self {
            kind: BodyKind::Text,
            expressions,
            motions: vec!["idle".into(), "nod".into(), "wave".into()],
            emotion_map,
        }
    }

    #[must_use]
    pub fn vrm_default() -> Self {
        let mut catalog = Self::text_default();
        catalog.kind = BodyKind::Vrm;
        catalog
    }

    /// Map a soul emotion onto this body. Missing labels nearest-fallback (P-402).
    pub fn map_emotion(
        &self,
        cue: &EmotionCue,
        fallback: &FallbackSettings,
    ) -> Result<(PerformanceCommand, Option<String>), BodyError> {
        let key = cue.label.trim().to_ascii_lowercase();
        if let Some(mapped) = self.emotion_map.get(&key) {
            return self.expression_cmd(&mapped.expression, cue.intensity * mapped.scale(), None);
        }
        if !fallback.nearest_expression {
            return Err(BodyError::UnknownExpression(key));
        }
        let nearest = nearest_key(&key, self.emotion_map.keys());
        let Some(mapped) = nearest.and_then(|k| self.emotion_map.get(k)) else {
            return Err(BodyError::UnknownExpression(key));
        };
        let warn = Some(format!(
            "emotion '{key}' missing from body map; using '{}'",
            mapped.expression
        ));
        self.expression_cmd(&mapped.expression, cue.intensity * mapped.scale(), warn)
    }

    fn expression_cmd(
        &self,
        expression: &str,
        intensity: f32,
        warning: Option<String>,
    ) -> Result<(PerformanceCommand, Option<String>), BodyError> {
        if !self.expressions.iter().any(|e| e == expression) {
            return Err(BodyError::UnknownExpression(expression.to_owned()));
        }
        Ok((
            PerformanceCommand::Expression {
                label: expression.to_owned(),
                intensity: intensity.clamp(0.0, 1.0),
                duration_ms: None,
            },
            warning,
        ))
    }

    pub fn validate_motion(&self, name: &str) -> Result<(), BodyError> {
        if self.motions.iter().any(|m| m == name) {
            Ok(())
        } else {
            Err(BodyError::UnknownMotion(name.to_owned()))
        }
    }
}

fn nearest_key<'a>(needle: &str, keys: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    keys.min_by_key(|label| {
        let mut score = needle.len().abs_diff(label.len()) * 4;
        for (ca, cb) in needle.chars().zip(label.chars()) {
            if ca != cb {
                score += 1;
            }
        }
        score
    })
}
