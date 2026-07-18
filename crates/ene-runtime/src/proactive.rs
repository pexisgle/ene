//! Proactive companion speech scheduling helpers (#166).

use ene_ai::LlmProvider;
use ene_mind::{
    ProactiveConfig, ProactiveObservation, ProactiveSuppressionState, build_proactive_context,
    decide_proactive_speech,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Mutable scheduler counters owned by the actor.
#[derive(Debug)]
pub(crate) struct ProactiveScheduler {
    /// Latest host observation.
    pub observation: ProactiveObservation,
    /// When the last user `Run` started.
    pub last_user_input_at: Instant,
    /// When the last proactive utterance completed.
    pub last_proactive_at: Option<Instant>,
    /// Proactive turns completed in this session.
    pub proactive_turns: usize,
    /// Bumped whenever a user turn starts so in-flight decisions are discarded.
    pub epoch: u64,
}

impl Default for ProactiveScheduler {
    fn default() -> Self {
        Self {
            observation: ProactiveObservation::default(),
            last_user_input_at: Instant::now(),
            last_proactive_at: None,
            proactive_turns: 0,
            epoch: 0,
        }
    }
}

impl ProactiveScheduler {
    /// Record that a user turn began (cancels stale decisions via epoch bump).
    pub fn on_user_turn_started(&mut self) {
        self.last_user_input_at = Instant::now();
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Record a completed proactive utterance.
    pub fn on_proactive_completed(&mut self) {
        self.last_proactive_at = Some(Instant::now());
        self.proactive_turns = self.proactive_turns.saturating_add(1);
    }

    /// Reset per-session counters (character / session change).
    pub fn reset_session(&mut self) {
        self.proactive_turns = 0;
        self.last_proactive_at = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.observation = ProactiveObservation::default();
        self.last_user_input_at = Instant::now();
    }

    /// Build suppression state for mind gates.
    #[must_use]
    pub fn suppression(&self, user_turn_busy: bool) -> ProactiveSuppressionState {
        let seconds_since_user_input = self.last_user_input_at.elapsed().as_secs();
        let seconds_since_proactive = self
            .last_proactive_at
            .map_or(u64::MAX, |t| t.elapsed().as_secs());
        ProactiveSuppressionState {
            seconds_since_user_input,
            seconds_since_proactive,
            proactive_turns_this_session: self.proactive_turns,
            user_turn_busy,
        }
    }
}

/// Result of an async decision task.
#[derive(Debug, Clone)]
pub(crate) struct ProactiveDecisionResult {
    /// Epoch captured when the decision started.
    pub epoch: u64,
    /// Whether generation should start.
    pub should_generate: bool,
    /// Topic hint for the generation prompt.
    pub topic_hint: String,
    /// Diagnostic reason / skip text.
    pub detail: String,
}

/// Run a decision against the current session snapshot (spawned off the actor).
pub(crate) async fn run_decision_task(
    config: ProactiveConfig,
    history: Vec<ene_mind::HistoryEntry>,
    observation: ProactiveObservation,
    suppression: ProactiveSuppressionState,
    provider: Option<Arc<dyn LlmProvider>>,
    epoch: u64,
) -> ProactiveDecisionResult {
    let context = build_proactive_context(&config, &history, &observation, None, &[], suppression);
    let outcome = decide_proactive_speech(&config, &context, provider).await;
    let should_generate = outcome.skip.is_none()
        && outcome
            .decision
            .allows_generation(config.decision.min_confidence);
    let detail = if let Some(skip) = &outcome.skip {
        format!("{skip:?}")
    } else {
        outcome.decision.reason.clone()
    };
    ProactiveDecisionResult {
        epoch,
        should_generate,
        topic_hint: outcome.decision.topic_hint,
        detail,
    }
}

/// Build the internal generation hint (never stored as a user message).
#[must_use]
pub(crate) fn proactive_generation_hint(topic_hint: &str) -> String {
    let topic = topic_hint.trim();
    if topic.is_empty() {
        "The user has been idle. Speak briefly and naturally as the companion — one short check-in. Do not invent that the user said something.".to_string()
    } else {
        format!(
            "The user has been idle. Speak briefly and naturally as the companion about: {topic}. Do not invent that the user said something. Do not quote internal decision reasons."
        )
    }
}

/// Interval duration from config (minimum 1s).
#[must_use]
pub(crate) fn tick_period(config: &ProactiveConfig) -> Duration {
    Duration::from_secs(config.interval_seconds.max(1))
}
