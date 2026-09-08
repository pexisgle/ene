//! Proactive speech: event-driven gates and quiet hours (D-28).

mod gate;
mod parse;
mod privacy;
mod quiet_hours;
mod world_state;

pub use gate::{GateRejectReason, evaluate_deterministic_gates};
pub use parse::{decision_schema_object, parse_decision_json};
pub use privacy::redact_window_title;
pub use quiet_hours::{QuietHoursEval, evaluate_quiet_hours};
pub use world_state::{IdleTrend, WorldStateMemory, WorldStateSnapshot, WorldStateSummary};

use crate::classify::{ClassifyModel, ClassifyTask};
use crate::config::ProactiveSettings;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// First-token refusal marker for integrated confirmation.
pub const SILENT_TOKEN: &str = "<|silent|>";

/// Privacy-safe activity snapshot from the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub idle_seconds: Option<u64>,
    pub active_window_label: String,
    pub recent_change: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSummaryStatus {
    #[default]
    Disabled,
    Unavailable,
    Available,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactiveObservation {
    pub captured_at_unix_ms: u64,
    pub activity: Option<ActivitySnapshot>,
    pub screen_summary: Option<String>,
    pub screen_summary_status: ScreenSummaryStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProactiveSuppressionState {
    pub seconds_since_user_input: u64,
    pub seconds_since_proactive: u64,
    pub proactive_turns_this_session: usize,
    pub user_turn_busy: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingConfirmationPrompt {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub age_days: f64,
}

#[derive(Debug, Clone)]
pub struct ProactiveContext {
    pub history: Vec<String>,
    pub seconds_since_user_input: u64,
    pub activity: Option<ActivitySnapshot>,
    pub screen_summary: Option<String>,
    pub affect_summary: Option<String>,
    pub fatigue: Option<f32>,
    pub commitments: Vec<String>,
    pub user_instructions: Vec<String>,
    pub suppression: ProactiveSuppressionState,
    pub quiet_hours: QuietHoursEval,
    pub pending_confirmation: Option<PendingConfirmationPrompt>,
    pub world_state: Option<WorldStateSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveUrgency {
    Low,
    #[default]
    Normal,
    High,
}

impl ProactiveUrgency {
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("low") => Self::Low,
            Some("high") => Self::High,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProactiveDecision {
    pub should_speak: bool,
    pub confidence: f64,
    pub screen_digest: String,
    pub reason: String,
    pub topic_hint: String,
    pub urgency: ProactiveUrgency,
}

impl ProactiveDecision {
    #[must_use]
    pub fn silent(reason: impl Into<String>) -> Self {
        Self {
            should_speak: false,
            confidence: 0.0,
            screen_digest: String::new(),
            reason: reason.into(),
            topic_hint: String::new(),
            urgency: ProactiveUrgency::Normal,
        }
    }

    #[must_use]
    pub fn allows_generation(&self, min_confidence: f64) -> bool {
        self.should_speak && self.confidence >= min_confidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSkipReason {
    Disabled,
    Gate(GateRejectReason),
    DecisionFailed(String),
    BelowConfidence,
    ModelDeclined,
    ConfirmationDeclined,
}

impl fmt::Display for ProactiveSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Gate(reason) => write!(f, "gate: {reason}"),
            Self::DecisionFailed(error) => write!(f, "decision failed: {error}"),
            Self::BelowConfidence => write!(f, "below confidence"),
            Self::ModelDeclined => write!(f, "model declined"),
            Self::ConfirmationDeclined => write!(f, "confirmation declined"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProactiveConfirmation {
    Disabled,
    Pending,
    Accepted,
    Declined,
    Empty,
}

#[derive(Debug, Clone)]
pub struct ProactiveDecisionOutcome {
    pub decision: ProactiveDecision,
    pub skip: Option<ProactiveSkipReason>,
    pub llm_invoked: bool,
    pub confirmation: ProactiveConfirmation,
}

#[must_use]
pub fn build_proactive_context(
    config: &ProactiveSettings,
    history: &[String],
    observation: &ProactiveObservation,
    affect_summary: Option<String>,
    fatigue: Option<f32>,
    commitments: &[String],
    user_instructions: &[String],
    suppression: ProactiveSuppressionState,
    quiet_hours: QuietHoursEval,
    pending_confirmation: Option<PendingConfirmationPrompt>,
    world_state: Option<&WorldStateMemory>,
) -> ProactiveContext {
    let history = if config.sources.conversation {
        truncate_history(history, config.max_conversation_chars)
    } else {
        Vec::new()
    };
    let activity = if config.sources.activity {
        observation.activity.as_ref().map(|snap| ActivitySnapshot {
            idle_seconds: snap.idle_seconds,
            active_window_label: truncate_chars(
                &redact_window_title(&snap.active_window_label, config.world_state.title_mode),
                config.max_activity_chars.min(200),
            ),
            recent_change: truncate_chars(&snap.recent_change, config.max_activity_chars),
        })
    } else {
        None
    };
    let screen_summary = if config.sources.screen_summary
        && observation.screen_summary_status == ScreenSummaryStatus::Available
    {
        observation
            .screen_summary
            .as_ref()
            .map(|s| truncate_chars(s, config.max_screen_summary_chars))
    } else {
        None
    };
    let user_instructions = if config.sources.memory {
        user_instructions
            .iter()
            .map(|line| truncate_chars(line, 160))
            .collect()
    } else {
        Vec::new()
    };
    let world_state = world_state.and_then(|memory| memory.summary(&config.world_state));
    ProactiveContext {
        history,
        seconds_since_user_input: suppression.seconds_since_user_input,
        activity,
        screen_summary,
        affect_summary,
        fatigue,
        commitments: commitments.iter().map(|c| truncate_chars(c, 160)).collect(),
        user_instructions,
        suppression,
        quiet_hours,
        pending_confirmation,
        world_state,
    }
}

/// Deterministic gates then optional classifier. Always fail-closed.
pub async fn decide_proactive_speech(
    config: &ProactiveSettings,
    context: &ProactiveContext,
    classifier: Option<&dyn ClassifyModel>,
) -> ProactiveDecisionOutcome {
    if !config.enabled {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent("proactive disabled"),
            skip: Some(ProactiveSkipReason::Disabled),
            llm_invoked: false,
            confirmation: ProactiveConfirmation::Disabled,
        };
    }
    if let Err(reason) = evaluate_deterministic_gates(config, context) {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent(reason.to_string()),
            skip: Some(ProactiveSkipReason::Gate(reason)),
            llm_invoked: false,
            confirmation: ProactiveConfirmation::Disabled,
        };
    }
    let Some(classifier) = classifier else {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent("decision provider unavailable"),
            skip: Some(ProactiveSkipReason::DecisionFailed(
                "decision provider unavailable".to_owned(),
            )),
            llm_invoked: false,
            confirmation: ProactiveConfirmation::Disabled,
        };
    };
    let input = serde_json::to_string(&decision_input(context)).unwrap_or_else(|_| "{}".to_owned());
    let timeout = Duration::from_secs(config.decision_timeout_seconds.max(1));
    let raw = match tokio::time::timeout(
        timeout,
        classifier.complete_json(ClassifyTask::ProactiveDecision, &input),
    )
    .await
    {
        Ok(Ok(text)) => text,
        Ok(Err(err)) => {
            return failed(err.to_string());
        }
        Err(_) => return failed("decision timed out".to_owned()),
    };
    let mut decision = parse_decision_json(&raw);
    if context.screen_summary.is_none() {
        decision.screen_digest.clear();
    }
    if !decision.allows_generation(config.effective_decision_min_confidence()) {
        let skip = Some(if decision.should_speak {
            ProactiveSkipReason::BelowConfidence
        } else {
            ProactiveSkipReason::ModelDeclined
        });
        return ProactiveDecisionOutcome {
            decision,
            skip,
            llm_invoked: true,
            confirmation: ProactiveConfirmation::Disabled,
        };
    }
    ProactiveDecisionOutcome {
        decision,
        skip: None,
        llm_invoked: true,
        confirmation: if config.confirmation_enabled {
            ProactiveConfirmation::Pending
        } else {
            ProactiveConfirmation::Disabled
        },
    }
}

fn failed(reason: String) -> ProactiveDecisionOutcome {
    ProactiveDecisionOutcome {
        decision: ProactiveDecision::silent(reason.clone()),
        skip: Some(ProactiveSkipReason::DecisionFailed(reason)),
        llm_invoked: true,
        confirmation: ProactiveConfirmation::Disabled,
    }
}

fn decision_input(context: &ProactiveContext) -> serde_json::Value {
    serde_json::json!({
        "history": context.history,
        "idle": context.seconds_since_user_input,
        "activity": context.activity,
        "screen_summary": context.screen_summary,
        "affect": context.affect_summary,
        "commitments": context.commitments,
        "user_instructions": context.user_instructions,
        "world_state": context.world_state,
        "pending_confirmation": context.pending_confirmation,
        "schema": decision_schema_object(),
    })
}

/// Classify a generation prefix as confirmation verdict.
#[must_use]
pub fn classify_confirmation_prefix(text: &str) -> ProactiveConfirmation {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return ProactiveConfirmation::Empty;
    }
    if trimmed.starts_with(SILENT_TOKEN) {
        return ProactiveConfirmation::Declined;
    }
    ProactiveConfirmation::Accepted
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if input.chars().count() <= max_chars {
        return input.to_owned();
    }
    input.chars().take(max_chars).collect()
}

fn truncate_history(history: &[String], max_chars: usize) -> Vec<String> {
    if max_chars == 0 || history.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    for entry in history.iter().rev() {
        let content = truncate_chars(entry, max_chars.saturating_sub(used).max(1));
        let cost = content.chars().count().saturating_add(8);
        if used.saturating_add(cost) > max_chars && !out.is_empty() {
            break;
        }
        used = used.saturating_add(cost);
        out.push(content);
    }
    out.reverse();
    out
}
