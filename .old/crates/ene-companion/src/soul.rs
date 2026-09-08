use crate::affect::{AffectBaseline, AffectState};
use crate::error::CompanionError;
use ene_session::{BodyId, SoulId};
use serde::{Deserialize, Serialize};

/// Runtime soul row in `companions.db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Soul {
    pub id: SoulId,
    pub character_ref: String,
    pub body_ref: Option<BodyId>,
    pub voice_ref: Option<String>,
    pub skill_refs: Vec<String>,
    pub affect_baseline: AffectBaseline,
    pub affect: AffectState,
    pub created_at: String,
    pub updated_at: String,
}

/// Fields required to insert a soul.
#[derive(Debug, Clone)]
pub struct NewSoul {
    pub character_ref: String,
    pub body_ref: Option<BodyId>,
    pub voice_ref: Option<String>,
    pub skill_refs: Vec<String>,
    pub affect_baseline: AffectBaseline,
}

impl NewSoul {
    #[must_use]
    pub fn text_only(character_ref: impl Into<String>) -> Self {
        Self {
            character_ref: character_ref.into(),
            body_ref: None,
            voice_ref: None,
            skill_refs: Vec::new(),
            affect_baseline: AffectBaseline::default(),
        }
    }
}

pub(crate) fn parse_skill_refs(raw: &str) -> Result<Vec<String>, CompanionError> {
    serde_json::from_str(raw).map_err(|err| CompanionError::codec(err.to_string()))
}
