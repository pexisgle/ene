use crate::config::ProactiveSettings;
use crate::proactive::ProactiveContext;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRejectReason {
    ManualPause,
    QuietHours,
    UserTurnBusy,
    MinIdle,
    Cooldown,
    SessionLimit,
    ActivityEngaged,
    NoSources,
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
            Self::ActivityEngaged => write!(f, "user is active at the keyboard"),
            Self::NoSources => write!(f, "no proactive sources available"),
            Self::HighFatigue => write!(f, "high fatigue"),
        }
    }
}

pub fn evaluate_deterministic_gates(
    config: &ProactiveSettings,
    context: &ProactiveContext,
) -> Result<(), GateRejectReason> {
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
    if config.world_state.enabled
        && let Some(world) = &context.world_state
        && (world.engaged || world.idle_trend == crate::proactive::IdleTrend::Falling)
    {
        return Err(GateRejectReason::ActivityEngaged);
    }
    let has_pending = context.pending_confirmation.is_some();
    if !config.sources.any_enabled() && !config.world_state.enabled && !has_pending {
        return Err(GateRejectReason::NoSources);
    }
    let has_conversation = config.sources.conversation && !context.history.is_empty();
    let has_activity = config.sources.activity && context.activity.is_some();
    let has_screen = config.sources.screen_summary && context.screen_summary.is_some();
    let has_instructions = config.sources.memory && !context.user_instructions.is_empty();
    let has_world = config.world_state.enabled && context.world_state.is_some();
    if !(has_conversation
        || has_activity
        || has_screen
        || has_instructions
        || has_world
        || has_pending)
    {
        return Err(GateRejectReason::NoSources);
    }
    if let Some(fatigue) = context.fatigue
        && fatigue >= config.fatigue_suppression_threshold
    {
        return Err(GateRejectReason::HighFatigue);
    }
    if context.quiet_hours.active && config.quiet_hours.suppress_decisions {
        return Err(GateRejectReason::QuietHours);
    }
    Ok(())
}
