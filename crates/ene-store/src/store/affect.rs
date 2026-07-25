//! Affect-state and pending-affect-proposal queries.

use super::{MemoryError, MemoryStore};
use crate::entities;
use chrono::Utc;
use sea_orm::EntityTrait;

impl MemoryStore {
    /// Retrieve the current [`crate::AffectState`] for a character.
    pub async fn get_affect_state(
        &self,
        character_id: &str,
    ) -> Result<crate::AffectState, MemoryError> {
        use entities::affect_states::Entity;
        use sea_orm::EntityTrait;

        let maybe_model = Entity::find_by_id(character_id).one(&self.db).await?;
        match maybe_model {
            Some(model) => {
                let discrete_emotions: Vec<crate::DiscreteEmotion> =
                    serde_json::from_str(&model.discrete_emotions).unwrap_or_else(|e| {
                        tracing::error!(
                            component = "MemoryStore",
                            character_id = %model.character_id,
                            error = %e,
                            "Failed to deserialize discrete_emotions, returning empty list"
                        );
                        Vec::new()
                    });
                Ok(crate::AffectState {
                    character_id: model.character_id,
                    user_id: model.user_id,
                    valence: model.valence,
                    arousal: model.arousal,
                    dominance: model.dominance,
                    trust: model.trust,
                    affinity: model.affinity,
                    irritation: model.irritation,
                    curiosity: model.curiosity,
                    fatigue: model.fatigue,
                    mood_label: model.mood_label,
                    last_expression: model.last_expression,
                    discrete_emotions,
                    updated_at: Some(model.updated_at),
                })
            }
            None => Ok(crate::AffectState::neutral(character_id)),
        }
    }

    /// Persist or update an [`crate::AffectState`].
    pub async fn upsert_affect_state(&self, state: &crate::AffectState) -> Result<(), MemoryError> {
        use entities::affect_states::{ActiveModel, Column, Entity};
        use sea_orm::sea_query::OnConflict;

        let mut state = state.clone();
        state.clamp();

        let now = Utc::now();
        let discrete_json = serde_json::to_string(&state.discrete_emotions)
            .map_err(|e| MemoryError::Other(e.to_string()))?;

        let active = ActiveModel {
            character_id: sea_orm::Set(state.character_id),
            user_id: sea_orm::Set(state.user_id),
            valence: sea_orm::Set(state.valence),
            arousal: sea_orm::Set(state.arousal),
            dominance: sea_orm::Set(state.dominance),
            trust: sea_orm::Set(state.trust),
            affinity: sea_orm::Set(state.affinity),
            irritation: sea_orm::Set(state.irritation),
            curiosity: sea_orm::Set(state.curiosity),
            fatigue: sea_orm::Set(state.fatigue),
            mood_label: sea_orm::Set(state.mood_label),
            last_expression: sea_orm::Set(state.last_expression),
            discrete_emotions: sea_orm::Set(discrete_json),
            updated_at: sea_orm::Set(now),
        };

        Entity::insert(active)
            .on_conflict(
                OnConflict::column(Column::CharacterId)
                    .update_columns([
                        Column::UserId,
                        Column::Valence,
                        Column::Arousal,
                        Column::Dominance,
                        Column::Trust,
                        Column::Affinity,
                        Column::Irritation,
                        Column::Curiosity,
                        Column::Fatigue,
                        Column::MoodLabel,
                        Column::LastExpression,
                        Column::DiscreteEmotions,
                        Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Upsert a pending post-turn classifier proposal for the next turn.
    pub async fn upsert_pending_affect_proposal(
        &self,
        proposal: &crate::PendingAffectProposal,
    ) -> Result<(), MemoryError> {
        use entities::pending_affect_proposals::{ActiveModel, Column, Entity};
        use sea_orm::sea_query::OnConflict;

        let proposal_json =
            serde_json::to_string(proposal).map_err(|e| MemoryError::Other(e.to_string()))?;

        let active = ActiveModel {
            character_id: sea_orm::Set(proposal.character_id.clone()),
            user_id: sea_orm::Set(proposal.user_id.clone()),
            source_turn_id: sea_orm::Set(proposal.source_turn_id),
            proposal_json: sea_orm::Set(proposal_json),
            created_at: sea_orm::Set(proposal.created_at),
        };

        Entity::insert(active)
            .on_conflict(
                OnConflict::columns([Column::CharacterId, Column::UserId])
                    .update_columns([
                        Column::SourceTurnId,
                        Column::ProposalJson,
                        Column::CreatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Retrieve a pending post-turn classifier proposal, if any.
    pub async fn get_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<Option<crate::PendingAffectProposal>, MemoryError> {
        use entities::pending_affect_proposals::Entity;
        use sea_orm::EntityTrait;

        let maybe_model = Entity::find_by_id((character_id.to_string(), user_id.to_string()))
            .one(&self.db)
            .await?;
        let Some(model) = maybe_model else {
            return Ok(None);
        };

        match serde_json::from_str::<crate::PendingAffectProposal>(&model.proposal_json) {
            Ok(proposal) => Ok(Some(proposal)),
            Err(error) => {
                tracing::warn!(
                    component = "MemoryStore",
                    character_id,
                    user_id,
                    error = %error,
                    "Dropping stale pending affect proposal with incompatible JSON"
                );
                self.delete_pending_affect_proposal(character_id, user_id)
                    .await?;
                Ok(None)
            }
        }
    }

    /// Delete a pending post-turn classifier proposal for a character/user key.
    pub async fn delete_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<(), MemoryError> {
        use entities::pending_affect_proposals::Entity;
        use sea_orm::EntityTrait;

        Entity::delete_by_id((character_id.to_string(), user_id.to_string()))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    /// Fetch and consume a pending post-turn classifier proposal.
    pub async fn take_pending_affect_proposal(
        &self,
        character_id: &str,
        user_id: &str,
    ) -> Result<Option<crate::PendingAffectProposal>, MemoryError> {
        let proposal = self
            .get_pending_affect_proposal(character_id, user_id)
            .await?;
        if proposal.is_some() {
            self.delete_pending_affect_proposal(character_id, user_id)
                .await?;
        }
        Ok(proposal)
    }
}
