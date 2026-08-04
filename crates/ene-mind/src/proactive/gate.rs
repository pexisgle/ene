//! Deterministic pre-LLM gates for proactive speech.

use crate::config::ProactiveConfig;
use crate::proactive::ProactiveContext;
use std::fmt;

/// Reasons a proactive tick is suppressed without calling the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRejectReason {
    /// Manual pause (`ProactiveConfig::paused`) is active.
    ManualPause,
    /// The configured quiet-hours window is active and suppresses decisions.
    QuietHours,
    /// A user turn / tool / permission wait is active.
    UserTurnBusy,
    /// Last user input was too recent.
    MinIdle,
    /// Cooldown after a previous proactive utterance.
    Cooldown,
    /// Session proactive turn cap reached.
    SessionLimit,
    /// Every configured input source is disabled or empty.
    NoSources,
    /// Character fatigue is at or above `fatigue_suppression_threshold`.
    HighFatigue,
}

impl fmt::Display for GateRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManualPause => write!(f, "manual pause"),
            Self::QuietHours => write!(f, "quiet hours"),
            Self::UserTurnBusy => write!(f, "user turn busy"),
            Self::MinIdle => write!(f, "min idle not reached"),
            Self::Cooldown => write!(f, "proactive cooldown"),
            Self::SessionLimit => write!(f, "session proactive limit"),
            Self::NoSources => write!(f, "no proactive sources available"),
            Self::HighFatigue => write!(f, "high fatigue"),
        }
    }
}

/// Evaluate deterministic gates. `Ok(())` means the LLM may be called.
pub fn evaluate_deterministic_gates(
    config: &ProactiveConfig,
    context: &ProactiveContext,
) -> Result<(), GateRejectReason> {
    // Manual pause outranks quiet hours and the counter gates: an explicit
    // user stop suppresses even outside the quiet-hours window.
    if config.paused {
        return Err(GateRejectReason::ManualPause);
    }
    if context.suppression.user_turn_busy {
        return Err(GateRejectReason::UserTurnBusy);
    }
    if context.suppression.seconds_since_user_input < config.min_idle_seconds {
        return Err(GateRejectReason::MinIdle);
    }
    if context.suppression.seconds_since_proactive < config.cooldown_seconds {
        return Err(GateRejectReason::Cooldown);
    }
    if context.suppression.proactive_turns_this_session >= config.max_turns_per_session {
        return Err(GateRejectReason::SessionLimit);
    }
    if !config.sources.any_enabled() {
        return Err(GateRejectReason::NoSources);
    }

    let has_conversation = config.sources.conversation && !context.history.is_empty();
    let has_activity = config.sources.activity && context.activity.is_some();
    let has_screen = config.sources.screen_summary && context.screen_summary.is_some();
    // User standing rules are decision input too: a memory-only configuration
    // still has something to decide on (and to be suppressed by).
    let has_instructions = config.sources.memory && !context.user_instructions.is_empty();
    if !(has_conversation || has_activity || has_screen || has_instructions) {
        return Err(GateRejectReason::NoSources);
    }

    // Unknown fatigue (no affect state) never suppresses: only a measured
    // value at or above the threshold does. The gate compares the unrounded
    // source value, not the prompt's `{:.2}` wire value, so it stays aligned
    // with `compute_mood_label`'s raw boundary instead of tripping on
    // round-trip rounding around the threshold.
    if let Some(fatigue) = context.fatigue
        && fatigue >= config.fatigue_suppression_threshold
    {
        return Err(GateRejectReason::HighFatigue);
    }

    // Quiet hours are evaluated last: the warrant gates above decide whether
    // an utterance opportunity exists at all, and quiet hours then decide
    // whether it may be delivered. This ordering lets callers treat a
    // `QuietHours` rejection as "the other gates would have passed".
    if context.quiet_hours.active && config.quiet_hours.suppress.decisions {
        return Err(GateRejectReason::QuietHours);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProactiveSourcesConfig;
    use crate::lifecycle::HistoryEntry;
    use crate::proactive::{ActivitySnapshot, ProactiveSuppressionState};
    use ene_ai::Role;

    fn ctx_with(suppression: ProactiveSuppressionState) -> ProactiveContext {
        ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hi".into(),
            }],
            seconds_since_user_input: suppression.seconds_since_user_input,
            activity: Some(ActivitySnapshot::default()),
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression,
            quiet_hours: crate::proactive::QuietHoursEval::inactive(),
        }
    }

    #[test]
    fn rejects_busy_and_idle() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 60,
            cooldown_seconds: 10,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };
        let busy = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 120,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: true,
        });
        assert_eq!(
            evaluate_deterministic_gates(&config, &busy),
            Err(GateRejectReason::UserTurnBusy)
        );

        let idle = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 10,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        assert_eq!(
            evaluate_deterministic_gates(&config, &idle),
            Err(GateRejectReason::MinIdle)
        );
    }

    #[test]
    fn manual_pause_outranks_quiet_hours_and_counters() {
        let config = ProactiveConfig {
            enabled: true,
            paused: true,
            ..ProactiveConfig::default()
        };
        let mut ctx = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 120,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        // Even inside an active quiet-hours window the pause reason wins.
        ctx.quiet_hours = crate::proactive::QuietHoursEval {
            active: true,
            ..crate::proactive::QuietHoursEval::inactive()
        };
        assert_eq!(
            evaluate_deterministic_gates(&config, &ctx),
            Err(GateRejectReason::ManualPause)
        );

        // Outside quiet hours the pause still suppresses.
        ctx.quiet_hours = crate::proactive::QuietHoursEval::inactive();
        assert_eq!(
            evaluate_deterministic_gates(&config, &ctx),
            Err(GateRejectReason::ManualPause)
        );
    }

    #[test]
    fn quiet_hours_is_reported_only_after_the_warrant_gates_pass() {
        let config = ProactiveConfig {
            enabled: true,
            ..ProactiveConfig::default()
        };
        let mut ctx = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 120,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        ctx.quiet_hours = crate::proactive::QuietHoursEval {
            active: true,
            ..crate::proactive::QuietHoursEval::inactive()
        };
        // A warrant gate that fails (cooldown here) reports its own reason,
        // not quiet hours: there was no utterance opportunity to suppress.
        assert_eq!(
            evaluate_deterministic_gates(&config, &ctx),
            Err(GateRejectReason::Cooldown)
        );

        ctx.suppression.seconds_since_proactive = 1000;
        assert_eq!(
            evaluate_deterministic_gates(&config, &ctx),
            Err(GateRejectReason::QuietHours)
        );

        // Outside the window the gates pass.
        ctx.quiet_hours = crate::proactive::QuietHoursEval::inactive();
        assert_eq!(evaluate_deterministic_gates(&config, &ctx), Ok(()));
    }

    #[test]
    fn quiet_hours_pass_when_decisions_stay_enabled() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            quiet_hours: crate::config::QuietHoursConfig {
                enabled: true,
                suppress: crate::config::QuietHoursSuppressConfig {
                    decisions: false,
                    ..crate::config::QuietHoursSuppressConfig::default()
                },
                ..crate::config::QuietHoursConfig::default()
            },
            ..ProactiveConfig::default()
        };
        let mut ctx = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 120,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        ctx.quiet_hours = crate::proactive::QuietHoursEval {
            active: true,
            ..crate::proactive::QuietHoursEval::inactive()
        };
        assert_eq!(evaluate_deterministic_gates(&config, &ctx), Ok(()));

        // Outside the window the gate also passes.
        ctx.quiet_hours = crate::proactive::QuietHoursEval::inactive();
        assert_eq!(evaluate_deterministic_gates(&config, &ctx), Ok(()));
    }

    #[test]
    fn rejects_high_fatigue_at_or_above_threshold() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };
        let mut tired = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 60,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        tired.fatigue = Some(0.85);
        assert_eq!(
            evaluate_deterministic_gates(&config, &tired),
            Err(GateRejectReason::HighFatigue)
        );

        // Exactly at the default 0.7 "tired" boundary: still suppressed.
        tired.fatigue = Some(0.7);
        assert_eq!(
            evaluate_deterministic_gates(&config, &tired),
            Err(GateRejectReason::HighFatigue)
        );
    }

    #[test]
    fn passes_when_fatigue_is_low_unknown_or_absent() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };
        let mut rested = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 60,
            seconds_since_proactive: 120,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        rested.fatigue = Some(0.60);
        assert_eq!(evaluate_deterministic_gates(&config, &rested), Ok(()));

        // Just below the threshold (0.699) must pass on the unrounded value:
        // the `{:.2}` wire form would round it up to 0.70 and suppress.
        rested.fatigue = Some(0.699);
        assert_eq!(evaluate_deterministic_gates(&config, &rested), Ok(()));

        // No affect state at all: the gate must pass.
        rested.fatigue = None;
        assert_eq!(evaluate_deterministic_gates(&config, &rested), Ok(()));

        // A threshold of 1.0 disables the gate even at extreme fatigue.
        let relaxed = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            fatigue_suppression_threshold: 1.0,
            ..ProactiveConfig::default()
        };
        rested.fatigue = Some(0.95);
        assert_eq!(evaluate_deterministic_gates(&relaxed, &rested), Ok(()));
    }

    #[test]
    fn memory_notes_alone_satisfy_the_source_gate() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            sources: ProactiveSourcesConfig {
                conversation: false,
                activity: false,
                screen_summary: false,
                ..ProactiveSourcesConfig::default()
            },
            ..ProactiveConfig::default()
        };
        let mut ctx = ctx_with(ProactiveSuppressionState {
            seconds_since_user_input: 60,
            seconds_since_proactive: 1000,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        });
        ctx.history.clear();
        ctx.activity = None;

        // Without stored instructions the memory source provides no context.
        ctx.user_instructions.clear();
        assert_eq!(
            evaluate_deterministic_gates(&config, &ctx),
            Err(GateRejectReason::NoSources)
        );

        // A stored standing rule is decision input on its own.
        ctx.user_instructions = vec!["don't talk while I work".into()];
        assert_eq!(evaluate_deterministic_gates(&config, &ctx), Ok(()));
    }
}
