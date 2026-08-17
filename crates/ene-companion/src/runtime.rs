use crate::affect::{
    AffectPresentation, ExpressionArbiter, apply_self_report, apply_turn_signals, project_decay,
};
use crate::classify::ClassifyModel;
use crate::config::{AffectSettings, MemoryApprovalSettings, MindSettings};
use crate::error::CompanionError;
use crate::inner::parse_emotion_report;
use crate::memory::{
    ArbitrateOutcome, apply_forget_request, arbitrate, extract_turn, recall_weights,
};
use crate::soul::Soul;
use crate::store::CompanionStore;
use chrono::Utc;
use ene_session::{InnerAspect, SoulId};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-process companion facade over `companions.db`.
pub struct CompanionRuntime {
    store: Arc<CompanionStore>,
    settings: MindSettings,
    arbiters: Mutex<HashMap<SoulId, ExpressionArbiter>>,
}

impl CompanionRuntime {
    #[must_use]
    pub fn new(store: Arc<CompanionStore>, settings: MindSettings) -> Self {
        Self {
            store,
            settings,
            arbiters: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn store(&self) -> Arc<CompanionStore> {
        Arc::clone(&self.store)
    }

    /// Decay + user signals + optional self-report. Returns surface presentation.
    pub fn on_user_turn(
        &self,
        soul_id: SoulId,
        user_text: &str,
        inner: &[(InnerAspect, String)],
    ) -> Result<Option<AffectPresentation>, CompanionError> {
        let mut soul = self
            .store
            .get_soul(soul_id)?
            .ok_or_else(|| CompanionError::UnknownSoul(soul_id.to_string()))?;
        let now = Utc::now();
        project_decay(
            &mut soul.affect,
            &soul.affect_baseline,
            &self.settings.affect,
            now,
        );
        apply_turn_signals(&mut soul.affect, user_text, None, &self.settings.affect);
        let mut presentation = None;
        for (aspect, body) in inner {
            if *aspect == InnerAspect::Emotion
                && let Some(report) = parse_emotion_report(body)
            {
                presentation = Some(apply_self_report(&mut soul.affect, &report, now));
            }
        }
        self.store.save_affect(soul_id, &soul.affect)?;
        if let Some(pres) = presentation {
            let mut arbiters = self.arbiters.lock();
            let arbiter = arbiters.entry(soul_id).or_default();
            return Ok(arbiter.decide(pres, &self.settings.affect, now, true));
        }
        Ok(None)
    }

    /// Background extraction after a terminal turn.
    pub async fn after_turn(
        &self,
        soul_id: SoulId,
        user_text: &str,
        assistant_text: &str,
        classifier: Option<&dyn ClassifyModel>,
    ) -> Result<Vec<ArbitrateOutcome>, CompanionError> {
        if apply_forget_request(
            &self.store,
            soul_id,
            user_text,
            self.settings.forgetting.mode,
        )? > 0
        {
            return Ok(Vec::new());
        }
        let cands = extract_turn(soul_id, user_text, assistant_text, classifier).await;
        let mut out = Vec::new();
        for cand in cands {
            out.push(arbitrate(
                &self.store,
                &cand,
                &self.settings.memory_approval,
            )?);
        }
        Ok(out)
    }

    pub fn soul(&self, id: SoulId) -> Result<Soul, CompanionError> {
        self.store
            .get_soul(id)?
            .ok_or_else(|| CompanionError::UnknownSoul(id.to_string()))
    }

    pub fn recall(
        &self,
        soul_id: SoulId,
        query: &str,
    ) -> Result<Vec<crate::memory::RecalledMemory>, CompanionError> {
        self.store.recall(
            soul_id,
            query,
            self.settings.recall.budget,
            &Utc::now().to_rfc3339(),
            recall_weights(&self.settings.recall),
        )
    }

    #[must_use]
    pub fn affect_settings(&self) -> &AffectSettings {
        &self.settings.affect
    }

    #[must_use]
    pub fn approval_settings(&self) -> &MemoryApprovalSettings {
        &self.settings.memory_approval
    }
}
