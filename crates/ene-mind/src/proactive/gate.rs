//! Deterministic pre-LLM gates for proactive speech.

use crate::config::ProactiveConfig;
use crate::proactive::ProactiveContext;
use std::fmt;

/// Reasons a proactive tick is suppressed without calling the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRejectReason {
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
}

impl fmt::Display for GateRejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserTurnBusy => write!(f, "user turn busy"),
            Self::MinIdle => write!(f, "min idle not reached"),
            Self::Cooldown => write!(f, "proactive cooldown"),
            Self::SessionLimit => write!(f, "session proactive limit"),
            Self::NoSources => write!(f, "no proactive sources available"),
        }
    }
}

/// Evaluate deterministic gates. `Ok(())` means the LLM may be called.
pub fn evaluate_deterministic_gates(
    config: &ProactiveConfig,
    context: &ProactiveContext,
) -> Result<(), GateRejectReason> {
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
    if !(has_conversation || has_activity || has_screen) {
        return Err(GateRejectReason::NoSources);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            commitments: vec![],
            suppression,
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
}
