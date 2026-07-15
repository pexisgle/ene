//! Performance Arbiter: mid-turn cue management with priority-based resolution (#129).
//!
//! Accepts [`PerformanceCue`]s arriving during a turn (stream markers, affect mapping)
//! and resolves the final set of cues at turn-end. Priority ordering:
//! `LlmCommand > LlmAdvisory > Affect > Hysteresis > Fallback`.

use crate::output::arbiter::affect_to_expression;
use crate::output::{CueSource, PerfKind, PerformanceCue};
use ene_store::AffectState;

/// Tracks a single cue slot with priority semantics.
#[derive(Debug, Clone)]
struct CueSlot {
    cue: PerformanceCue,
    source: CueSource,
}

impl CueSlot {
    const fn new(cue: PerformanceCue, source: CueSource) -> Self {
        Self { cue, source }
    }

    /// Returns `true` if `incoming` should replace this slot.
    const fn should_replace(&self, incoming: CueSource) -> bool {
        cue_source_priority(incoming) >= cue_source_priority(self.source)
    }
}

/// Map a [`CueSource`] to its numeric priority (higher = more important).
#[must_use]
pub const fn cue_source_priority(source: CueSource) -> u8 {
    match source {
        CueSource::LlmCommand => 5,
        CueSource::LlmAdvisory => 4,
        CueSource::Affect => 3,
        CueSource::Hysteresis => 2,
        CueSource::Fallback => 1,
    }
}

/// Mid-turn performance arbiter.
///
/// Buffers incoming [`PerformanceCue`]s from various sources
/// (stream markers, affect engine, etc.) and resolves the final
/// set at turn-end.
#[derive(Debug, Default)]
pub struct PerformanceArbiter {
    expression: Option<CueSlot>,
    motion: Option<CueSlot>,
    lookat: Option<CueSlot>,
}

impl PerformanceArbiter {
    /// Accept a cue from a mid-turn source (stream marker, affect, etc.).
    ///
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
            PerfKind::Motion => Self::set_slot(&mut self.motion, slot),
            PerfKind::LookAt => Self::set_slot(&mut self.lookat, slot),
            PerfKind::Cancel => {
                tracing::warn!(
                    component = "PerformanceArbiter",
                    "Cancel variant reached match arm (should be handled by early return above)"
                );
            }
        }
    }

    /// Set the affect-default expression if no LLM-driven expression is active.
    ///
    /// Also re-evaluates when the existing slot is itself an `Affect`-sourced
    /// cue (handles affect state changes across the turn).
    pub fn set_affect_default(&mut self, affect: &AffectState) {
        let name = affect_to_expression(affect).to_string();
        let needs_update = match &self.expression {
            None => true,
            Some(s) => {
                matches!(
                    s.source,
                    CueSource::Fallback | CueSource::Hysteresis | CueSource::Affect
                )
            }
        };
        if needs_update {
            let cue = PerformanceCue::expression(name);
            self.expression = Some(CueSlot::new(cue, CueSource::Affect));
        }
    }

    /// Resolve and drain the current state, returning the final set of cues
    /// with their resolved sources.
    ///
    /// Clears internal state after resolution (ready for next turn).
    #[must_use]
    pub fn resolve(&mut self) -> Vec<(PerformanceCue, CueSource)> {
        let mut result: Vec<(PerformanceCue, CueSource)> = Vec::with_capacity(3);

        for slot in [
            self.expression.take(),
            self.motion.take(),
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

    /// Returns the current expression name, if any.
    #[must_use]
    pub fn current_expression(&self) -> Option<&str> {
        self.expression.as_ref().map(|s| s.cue.name.as_str())
    }

    /// Returns the current motion name, if any.
    #[must_use]
    pub fn current_motion(&self) -> Option<&str> {
        self.motion.as_ref().map(|s| s.cue.name.as_str())
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
                self.motion = None;
                tracing::debug!(component = "PerformanceArbiter", "Motion cancelled");
            }
            "all" => {
                self.expression = None;
                self.motion = None;
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
    use super::*;

    fn expr_cue(name: &str) -> PerformanceCue {
        PerformanceCue::expression(name)
    }

    fn motion_cue(name: &str) -> PerformanceCue {
        PerformanceCue::motion(name, None)
    }

    #[test]
    fn command_overrides_advisory() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("sad"), CueSource::LlmAdvisory);
        arbiter.accept(expr_cue("angry"), CueSource::LlmCommand);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "angry");
        assert_eq!(result[0].1, CueSource::LlmCommand);
    }

    #[test]
    fn advisory_does_not_override_command() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("angry"), CueSource::LlmCommand);
        arbiter.accept(expr_cue("happy"), CueSource::LlmAdvisory);
        let result = arbiter.resolve();
        assert_eq!(result[0].0.name, "angry");
        assert_eq!(result[0].1, CueSource::LlmCommand);
    }

    #[test]
    fn cancel_clears_expression() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::LlmCommand);
        arbiter.accept(PerformanceCue::cancel("expr"), CueSource::LlmCommand);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }

    #[test]
    fn cancel_all_clears_everything() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::LlmCommand);
        arbiter.accept(motion_cue("wave"), CueSource::LlmCommand);
        arbiter.accept(PerformanceCue::cancel("all"), CueSource::LlmCommand);
        let result = arbiter.resolve();
        assert!(result.is_empty());
    }

    #[test]
    fn affect_fills_empty_slot() {
        let mut arbiter = PerformanceArbiter::default();
        let mut state = AffectState::neutral("test");
        state.valence = 0.5;
        state.arousal = 0.3;
        arbiter.set_affect_default(&state);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.name, "happy");
    }

    #[test]
    fn multiple_kinds_coexist() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::LlmCommand);
        arbiter.accept(motion_cue("wave"), CueSource::LlmCommand);
        arbiter.accept(PerformanceCue::look_at("user"), CueSource::LlmAdvisory);
        let result = arbiter.resolve();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn latest_same_priority_wins() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::LlmCommand);
        arbiter.accept(expr_cue("sad"), CueSource::LlmCommand);
        let result = arbiter.resolve();
        assert_eq!(result[0].0.name, "sad");
    }

    #[test]
    fn resolve_clears_state() {
        let mut arbiter = PerformanceArbiter::default();
        arbiter.accept(expr_cue("happy"), CueSource::LlmCommand);
        let _ = arbiter.resolve();
        let result2 = arbiter.resolve();
        assert!(result2.is_empty());
    }
}
