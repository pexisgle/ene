//! Performance Arbiter: mid-turn cue management with priority-based resolution.
//!
//! Priority ordering: `Llm > Affect > Hysteresis > Fallback`.
//! Expression cues should be accepted only from end-of-turn
//! [`crate::output::arbiter::resolve_expression`]; mid-turn stream markers
//! feed that resolver as proposals instead of permanently winning this slot.

use crate::output::arbiter::affect_to_expression;
use crate::output::{CueSource, MotionLayer, PerfKind, PerformanceCue};
use ene_card::ResolvedExpression;
use ene_core::AffectState;

#[derive(Debug, Clone)]
struct CueSlot {
    cue: PerformanceCue,
    source: CueSource,
}

impl CueSlot {
    const fn new(cue: PerformanceCue, source: CueSource) -> Self {
        Self { cue, source }
    }

    const fn should_replace(&self, incoming: CueSource) -> bool {
        cue_source_priority(incoming) >= cue_source_priority(self.source)
    }
}

/// Map a [`CueSource`] to its numeric priority (higher = more important).
pub const fn cue_source_priority(source: CueSource) -> u8 {
    match source {
        CueSource::Llm => 4,
        CueSource::Affect => 3,
        CueSource::Hysteresis => 2,
        CueSource::Fallback => 1,
    }
}

/// Mid-turn performance arbiter.
///
/// Buffers incoming [`PerformanceCue`]s from various sources
/// (stream markers, affect engine, etc.) and resolves the final
/// set at turn-end. Motion cues are routed to per-layer slots
/// so that Upper and Lower body motions can coexist.
#[derive(Debug, Default)]
pub struct PerformanceArbiter {
    expression: Option<CueSlot>,
    motion_upper: Option<CueSlot>,
    motion_lower: Option<CueSlot>,
    motion_full: Option<CueSlot>,
    lookat: Option<CueSlot>,
}

impl PerformanceArbiter {
    /// Cancel cues clear the relevant slot. Higher-priority sources
    /// replace lower-priority ones; equal-priority sources replace
    /// (latest wins).
    pub fn accept(&mut self, cue: PerformanceCue, source: CueSource) {
        if cue.kind == PerfKind::Cancel {
            self.apply_cancel(&cue);
            return;
        }
        let slot = CueSlot::new(cue, source);
        match slot.cue.kind {
            PerfKind::Expression => Self::set_slot(&mut self.expression, slot),
            PerfKind::Motion => {
                let layer = slot.cue.motion_layer.unwrap_or(MotionLayer::Full);
                match layer {
                    MotionLayer::Upper => Self::set_slot(&mut self.motion_upper, slot),
                    MotionLayer::Lower => Self::set_slot(&mut self.motion_lower, slot),
                    MotionLayer::Full => Self::set_slot(&mut self.motion_full, slot),
                }
            }
            PerfKind::LookAt => Self::set_slot(&mut self.lookat, slot),
            PerfKind::Cancel => {
                tracing::warn!(
                    component = "PerformanceArbiter",
                    "Cancel variant reached match arm (should be handled by early return above)"
                );
            }
        }
    }

    /// Set the affect-default expression when the expression slot is empty.
    ///
    /// Final expression decisions come from
    /// [`crate::output::arbiter::resolve_expression`]; this only fills gaps
    /// (e.g. emotion disabled or no resolve path ran). The affect mapping is
    /// used when it yields a confident match; otherwise a card-defined
    /// "neutral" fills the slot so the face does not freeze. When the card has
    /// neither, the slot stays empty and the previous expression is preserved.
    pub fn set_affect_default(&mut self, affect: &AffectState, available: &[ResolvedExpression]) {
        if self.expression.is_some() {
            return;
        }
        let name = if let Some(name) = affect_to_expression(affect, available) {
            name.to_string()
        } else {
            let Some(neutral) = available
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case("neutral"))
            else {
                return;
            };
            neutral.name.clone()
        };
        let cue = PerformanceCue::expression(name);
        self.expression = Some(CueSlot::new(cue, CueSource::Affect));
    }

    /// Clears internal state after resolution (ready for next turn).
    pub fn resolve(&mut self) -> Vec<(PerformanceCue, CueSource)> {
        let mut result: Vec<(PerformanceCue, CueSource)> = Vec::with_capacity(5);

        for slot in [
            self.expression.take(),
            self.motion_upper.take(),
            self.motion_lower.take(),
            self.motion_full.take(),
            self.lookat.take(),
        ]
        .into_iter()
        .flatten()
        {
            let express_val = slot.cue.name.clone();
            let source = slot.source;
            let resolved_source = if matches!(source, CueSource::Affect) && express_val == "neutral"
            {
                CueSource::Fallback
            } else {
                source
            };
            result.push((slot.cue, resolved_source));
        }

        result
    }

    pub fn current_expression(&self) -> Option<&str> {
        self.expression.as_ref().map(|s| s.cue.name.as_str())
    }

    /// Searches Full → Upper → Lower in priority order.
    pub fn current_motion(&self) -> Option<&str> {
        self.motion_full
            .as_ref()
            .map(|s| s.cue.name.as_str())
            .or_else(|| self.motion_upper.as_ref().map(|s| s.cue.name.as_str()))
            .or_else(|| self.motion_lower.as_ref().map(|s| s.cue.name.as_str()))
    }

    fn set_slot(target: &mut Option<CueSlot>, slot: CueSlot) {
        let kind = slot.cue.kind;
        let name = slot.cue.name.clone();
        let incoming_source = slot.source;
        let replaced = match target {
            Some(existing) if existing.should_replace(incoming_source) => {
                *target = Some(slot);
                true
            }
            None => {
                *target = Some(slot);
                true
            }
            _ => false,
        };
        if replaced {
            tracing::debug!(
                component = "PerformanceArbiter",
                kind = ?kind,
                name = %name,
                source = ?incoming_source,
                "Cue accepted"
            );
        } else {
            tracing::debug!(
                component = "PerformanceArbiter",
                kind = ?kind,
                name = %name,
                source = ?incoming_source,
                "Cue rejected (lower priority)"
            );
        }
    }

    fn apply_cancel(&mut self, cue: &PerformanceCue) {
        let scope = cue.name.to_ascii_lowercase();
        match scope.as_str() {
            "expr" | "expression" => {
                self.expression = None;
                tracing::debug!(component = "PerformanceArbiter", "Expression cancelled");
            }
            "motion" => {
                self.motion_upper = None;
                self.motion_lower = None;
                self.motion_full = None;
                tracing::debug!(component = "PerformanceArbiter", "All motions cancelled");
            }
            "all" => {
                self.expression = None;
                self.motion_upper = None;
                self.motion_lower = None;
                self.motion_full = None;
                self.lookat = None;
                tracing::debug!(component = "PerformanceArbiter", "All cues cancelled");
            }
            unknown => {
                tracing::debug!(
                    component = "PerformanceArbiter",
                    scope = %unknown,
                    "Unknown cancel scope; ignoring"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        clippy::default_trait_access,
        reason = "tests index into fixed-size fixture vectors and use explicit Default for fixture clarity"
    )]
    use super::*;
    use ene_card::{CharacterCardV3, resolve_expressions};

    fn expr_cue(name: &str) -> PerformanceCue {
        PerformanceCue::expression(name)
    }

    fn motion_cue(name: &str, layer: Option<MotionLayer>) -> PerformanceCue {
        PerformanceCue::motion(name, layer)
    }

    /// The production built-in defaults (via the real merge), so tests cannot
    /// drift from what a default card resolves to at runtime.
    fn annotated_defaults() -> Vec<ResolvedExpression> {
        resolve_expressions(&CharacterCardV3::default())
    }

    #[test]
    fn llm_overrides_affect() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("sad"), CueSource::Affect);
        arbiter.accept(expr_cue("angry"), CueSource::Llm);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "angry");
        assert_eq!(result[0].1, CueSource::Llm);
    }

    #[test]
    fn affect_does_not_override_llm() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("angry"), CueSource::Llm);
        arbiter.accept(expr_cue("happy"), CueSource::Affect);
        let result = arbiter.resolve();
        assert_eq!(result[0].0.name, "angry");
        assert_eq!(result[0].1, CueSource::Llm);
    }

    #[test]
    fn final_affect_decision_holds_without_mid_turn_expression() {
        // Expression markers are not accepted mid-turn; only the end-of-turn
        // resolve result occupies the expression slot.
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(motion_cue("wave", Some(MotionLayer::Upper)), CueSource::Llm);
        arbiter.accept(expr_cue("sad"), CueSource::Affect);
        let result = arbiter.resolve();
        let expr = result
            .iter()
            .find(|(cue, _)| cue.kind == PerfKind::Expression)
            .expect("expression cue");
        assert_eq!(expr.0.name, "sad");
        assert_eq!(expr.1, CueSource::Affect);
    }

    #[test]
    fn cancel_clears_expression() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::Llm);
        arbiter.accept(PerformanceCue::cancel("expr"), CueSource::Llm);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }

    #[test]
    fn cancel_all_clears_everything() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::Llm);
        arbiter.accept(motion_cue("wave", Some(MotionLayer::Upper)), CueSource::Llm);
        arbiter.accept(PerformanceCue::cancel("all"), CueSource::Llm);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }

    #[test]
    fn affect_fills_empty_slot() {
        let mut arbiter = PerformanceArbiter::default();
        let mut state = AffectState::neutral("test");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available = annotated_defaults();
        arbiter.set_affect_default(&state, &available);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "happy");
    }

    #[test]
    fn set_affect_default_does_not_overwrite_resolved() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("sad"), CueSource::Hysteresis);
        let mut state = AffectState::neutral("test");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available = annotated_defaults();
        arbiter.set_affect_default(&state, &available);
        let result = arbiter.resolve();
        assert_eq!(result[0].0.name, "sad");
        assert_eq!(result[0].1, CueSource::Hysteresis);
    }

    #[test]
    fn set_affect_default_emits_neutral_when_card_has_one() {
        // Unannotated card with a neutral-named expression: the resting face
        // is emitted instead of freezing the previous expression.
        let mut arbiter = PerformanceArbiter::default();
        let mut state = AffectState::neutral("test");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available: Vec<ResolvedExpression> = ["neutral", "smile", "frown"]
            .into_iter()
            .map(|name| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
                affect: None,
            })
            .collect();
        arbiter.set_affect_default(&state, &available);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "neutral");
        assert_eq!(result[0].1, CueSource::Fallback);
    }

    #[test]
    fn set_affect_default_skips_cards_without_neutral_or_annotations() {
        let mut arbiter = PerformanceArbiter::default();
        let mut state = AffectState::neutral("test");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available: Vec<ResolvedExpression> = ["smile", "frown"]
            .into_iter()
            .map(|name| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
                affect: None,
            })
            .collect();
        arbiter.set_affect_default(&state, &available);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }

    #[test]
    fn set_affect_default_resting_state_uses_neutral() {
        // Regression: an all-zero state must not default to a sad face.
        let mut arbiter = PerformanceArbiter::default();
        let state = AffectState::neutral("test");
        let available = annotated_defaults();
        arbiter.set_affect_default(&state, &available);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "neutral");
    }

    #[test]
    fn multiple_kinds_coexist() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::Llm);
        arbiter.accept(motion_cue("wave", Some(MotionLayer::Upper)), CueSource::Llm);
        arbiter.accept(PerformanceCue::look_at("user"), CueSource::Llm);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn latest_same_priority_wins() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::Llm);
        arbiter.accept(expr_cue("sad"), CueSource::Llm);
        let result = arbiter.resolve();
        assert_eq!(result[0].0.name, "sad");
    }

    #[test]
    fn resolve_clears_state() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::Llm);
        let _ = arbiter.resolve();
        let result2 = arbiter.resolve();
        assert!(result2.is_empty());
    }

    #[test]
    fn upper_and_lower_motion_coexist() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(motion_cue("wave", Some(MotionLayer::Upper)), CueSource::Llm);
        arbiter.accept(motion_cue("idle", Some(MotionLayer::Lower)), CueSource::Llm);
        let result = arbiter.resolve();
        let motion_names: Vec<&str> = result
            .iter()
            .filter(|(cue, _)| cue.kind == PerfKind::Motion)
            .map(|(cue, _)| cue.name.as_str())
            .collect();
        assert_eq!(motion_names.len(), 2);
        assert!(motion_names.contains(&"wave"));
        assert!(motion_names.contains(&"idle"));
    }

    #[test]
    fn cancel_motion_clears_all_layers() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(motion_cue("wave", Some(MotionLayer::Upper)), CueSource::Llm);
        arbiter.accept(motion_cue("idle", Some(MotionLayer::Lower)), CueSource::Llm);
        arbiter.accept(PerformanceCue::cancel("motion"), CueSource::Llm);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }
}
