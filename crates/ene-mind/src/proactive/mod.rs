//! Proactive companion speech decision pipeline.
//!
//! Builds a normalized [`ProactiveContext`], applies deterministic gates, asks a
//! lightweight LLM for structured JSON only, and fail-closes on any error.

mod gate;
mod parse;
mod pending_confirmation;
mod prompt;
mod quiet_hours;
mod world_state;

pub use gate::{GateRejectReason, evaluate_deterministic_gates};
pub use parse::{decision_schema_object, parse_decision_json};
pub use pending_confirmation::{
    PendingConfirmationPrompt, PendingResolutionVerdict, build_resolution_messages,
    load_due_pending_confirmation, parse_resolution_json, resolution_schema_object,
    select_due_pending_candidate,
};
pub use prompt::build_decision_messages;
pub use quiet_hours::{QuietHoursEval, evaluate_quiet_hours};
pub use world_state::{IdleTrend, WorldStateMemory, WorldStateSnapshot, WorldStateSummary};

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ene_ai::LlmProvider;
use ene_core::{ActiveCommitmentPrompt, AffectState, MemoryKind, MemoryPort, MemoryStatus};
use serde::{Deserialize, Serialize};

use crate::config::ProactiveConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;

/// First-token refusal marker the main generation model emits when it declines
/// a proactive utterance during integrated confirmation.
///
/// The stream cancels as soon as this marker appears before any visible text.
/// The prompt library's `confirmation_note` instructs the model to emit it
/// first; the token literal is duplicated there on purpose and locked by
/// tests on both sides.
pub const SILENT_TOKEN: &str = "<|silent|>";

/// Privacy-safe activity snapshot from the host (desktop).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub idle_seconds: Option<u64>,
    /// Privacy-aware label for the focused window.
    ///
    /// Depending on the configured `mind.proactive.sources.window_title_level`
    /// this is the app name only, the app name plus a redacted window title,
    /// or the app name plus the raw title. It is re-redacted defensively in
    /// [`build_proactive_context`] before reaching the decision prompt.
    pub active_window_label: String,
    /// Short description of the change since the previous observation.
    ///
    /// Empty when nothing changed; otherwise a phrase such as
    /// `"focused firefox"` or `"switched from firefox to code"`.
    pub recent_change: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSummaryStatus {
    #[default]
    Disabled,
    /// Source enabled but no summarizer is available on this host.
    Unavailable,
    /// Short-lived text summary (never raw image bytes).
    Available,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactiveObservation {
    /// When this observation was captured (unix millis).
    pub captured_at_unix_ms: u64,
    pub activity: Option<ActivitySnapshot>,
    /// Short-lived screen **text** summary when [`ScreenSummaryStatus::Available`].
    pub screen_summary: Option<String>,
    pub screen_summary_status: ScreenSummaryStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProactiveSuppressionState {
    pub seconds_since_user_input: u64,
    /// Seconds since the last successful proactive utterance (or `u64::MAX` if none).
    pub seconds_since_proactive: u64,
    pub proactive_turns_this_session: usize,
    /// True while a user turn, tool call, or permission/input wait is active.
    pub user_turn_busy: bool,
}

#[derive(Debug, Clone)]
pub struct ProactiveContext {
    /// Recent history entries already truncated for the prompt budget.
    pub history: Vec<HistoryEntry>,
    pub seconds_since_user_input: u64,
    pub activity: Option<ActivitySnapshot>,
    pub screen_summary: Option<String>,
    pub affect_summary: Option<String>,
    /// Unrounded fatigue from the affect state, compared by the deterministic
    /// gate instead of the prompt's two-decimal wire value.
    pub fatigue: Option<f32>,
    pub commitments: Vec<String>,
    /// User-stored standing rules one-liners (optional).
    ///
    /// `Preference` / `UserProfile` memories ("don't talk while I work", …)
    /// loaded deterministically — never through recall score competition — so
    /// a suppression condition cannot be dropped by a low score. Serialized
    /// as the trusted `user_instructions` field of the decision context.
    pub user_instructions: Vec<String>,
    pub suppression: ProactiveSuppressionState,
    /// Quiet-hours window evaluation for the decision instant. Inactive when
    /// quiet hours are disabled or outside the window.
    pub quiet_hours: QuietHoursEval,
    /// Overdue unconfirmed memory candidate selected for confirmation, if any.
    ///
    /// When present, the decision model judges whether now is a good moment to
    /// ask about it; the generation hint then renders a confirmation question
    /// instead of a topic hint.
    pub pending_confirmation: Option<PendingConfirmationPrompt>,
    /// Recent world-state trend summary when world-state memory is enabled
    /// and has enough history. `None` otherwise.
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
    fn parse(raw: Option<&str>) -> Self {
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
    /// Model confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Reorganized screen sketch (diagnostics only; never spoken verbatim).
    pub screen_digest: String,
    /// Internal reason (diagnostics only; never spoken verbatim).
    pub reason: String,
    pub topic_hint: String,
    pub urgency: ProactiveUrgency,
}

impl ProactiveDecision {
    /// Fail-closed "do not speak" decision.
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

/// Why a proactive decision was skipped or silenced.
///
/// A `None` `skip` means the decision led to speaking; every non-speech path
/// yields one of these variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSkipReason {
    Disabled,
    Gate(GateRejectReason),
    /// Decision model / parse / timeout failed (fail-closed).
    DecisionFailed(String),
    BelowConfidence,
    ModelDeclined,
    /// The main model declined during integrated confirmation; the generation
    /// stream was cancelled before any visible text.
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

/// Main-model confirmation state for a proactive decision.
///
/// The verdict is only observable while the generation stream runs, so the
/// decision stage records the initial state and the runtime resolves it to
/// [`ProactiveConfirmation::Accepted`] or [`ProactiveConfirmation::Declined`]
/// once the stream ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProactiveConfirmation {
    /// Confirmation was not requested: either the config flag is off or the
    /// pipeline stopped before generation started.
    Disabled,
    /// The decision passed the gate with confirmation enabled and generation
    /// is about to start; the verdict is not known yet.
    Pending,
    /// The main model accepted: generation completed with visible speech.
    Accepted,
    /// The main model declined with the refusal token; the stream was
    /// cancelled before any visible text.
    Declined,
    /// The main model produced no visible text (empty or marker-only
    /// response): neither an acceptance nor a decline.
    Empty,
}

impl fmt::Display for ProactiveConfirmation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Declined => write!(f, "declined"),
            Self::Empty => write!(f, "empty"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProactiveDecisionOutcome {
    /// Normalized decision (always present; fail-closed on errors).
    pub decision: ProactiveDecision,
    pub skip: Option<ProactiveSkipReason>,
    pub llm_invoked: bool,
    /// Main-model confirmation state at decision time.
    ///
    /// `Disabled` when the feature is off or the pipeline stopped before
    /// generation; `Pending` when the decision passed the gate with
    /// confirmation enabled. The runtime records the final verdict on its
    /// result and logs.
    pub confirmation: ProactiveConfirmation,
}

#[must_use]
pub fn build_proactive_context(
    config: &ProactiveConfig,
    history: &[HistoryEntry],
    observation: &ProactiveObservation,
    affect: Option<&AffectState>,
    commitments: &[ActiveCommitmentPrompt],
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
                &redact_window_label(&snap.active_window_label),
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

    let affect_summary = affect.map(|a| {
        format!(
            "mood={} valence={:.2} arousal={:.2} dominance={:.2} trust={:.2} \
             affinity={:.2} irritation={:.2} curiosity={:.2} fatigue={:.2}",
            a.mood_label,
            a.valence,
            a.arousal,
            a.dominance,
            a.trust,
            a.affinity,
            a.irritation,
            a.curiosity,
            a.fatigue
        )
    });

    let fatigue = affect.map(|a| a.fatigue);

    let commitments = commitments
        .iter()
        .map(|c| {
            let line = if c.description.is_empty() {
                c.title.clone()
            } else {
                format!("{}: {}", c.title, c.description)
            };
            truncate_chars(&line, 160)
        })
        .collect();

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
        commitments,
        user_instructions,
        suppression,
        quiet_hours,
        pending_confirmation,
        world_state,
    }
}

/// Deterministically load user standing-rule one-liners for a proactive decision.
///
/// Reads the user's `Preference` / `UserProfile` memories directly from the
/// store — bypassing hybrid recall scoring entirely — so a suppression
/// condition ("don't talk while I work") can never be dropped by a low recall
/// score. User scope is honored (plus character-level rows that carry no user
/// id), `Active` status only, newest first, capped at `max_notes`. The user
/// and status filters run in the store query, so rows that do not apply to
/// this user can never fill the newest-first window ahead of their rules.
pub async fn load_proactive_memory_notes(
    store: &dyn MemoryPort,
    character_id: &str,
    user_id: &str,
    max_notes: usize,
) -> Result<Vec<String>, CognitionError> {
    if max_notes == 0 {
        return Ok(Vec::new());
    }
    let mut notes = Vec::new();
    for kind in [MemoryKind::Preference, MemoryKind::UserProfile] {
        if notes.len() >= max_notes {
            break;
        }
        let rows = store
            .get_typed_memories_by_character(
                character_id,
                Some(kind),
                Some(user_id),
                Some(MemoryStatus::Active),
                max_notes,
                0,
            )
            .await
            .map_err(CognitionError::MemoryPort)?;
        for row in rows {
            if notes.len() >= max_notes {
                break;
            }
            // A note without title or content would inject a blank line and
            // still count as an instruction for the NoSources gate.
            if row.title.trim().is_empty() && row.content.trim().is_empty() {
                continue;
            }
            let line = if row.content.trim().is_empty() {
                row.title
            } else {
                format!("{}: {}", row.title, row.content)
            };
            notes.push(truncate_chars(&line, 160));
        }
    }
    Ok(notes)
}

/// Run deterministic gates + optional LLM decision. Always fail-closed.
pub async fn decide_proactive_speech(
    config: &ProactiveConfig,
    context: &ProactiveContext,
    provider: Option<Arc<dyn LlmProvider>>,
    prompt_language: &str,
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

    let Some(provider) = provider else {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent("decision provider unavailable"),
            skip: Some(ProactiveSkipReason::DecisionFailed(
                "decision provider unavailable".to_string(),
            )),
            llm_invoked: false,
            confirmation: ProactiveConfirmation::Disabled,
        };
    };

    let messages = build_decision_messages(context, prompt_language);
    // Plain JSON Schema only — providers wrap this as response_format themselves.
    let schema = decision_schema_object();
    let timeout = Duration::from_secs(config.decision_timeout_seconds.max(1));

    let raw = match tokio::time::timeout(timeout, provider.chat_completion(&messages, Some(schema)))
        .await
    {
        Ok(Ok(completion)) => completion.text,
        Ok(Err(e)) => {
            return ProactiveDecisionOutcome {
                decision: ProactiveDecision::silent(format!("decision provider error: {e}")),
                skip: Some(ProactiveSkipReason::DecisionFailed(e.to_string())),
                llm_invoked: true,
                confirmation: ProactiveConfirmation::Disabled,
            };
        }
        Err(_) => {
            return ProactiveDecisionOutcome {
                decision: ProactiveDecision::silent("decision timed out"),
                skip: Some(ProactiveSkipReason::DecisionFailed(
                    "decision timed out".to_string(),
                )),
                llm_invoked: true,
                confirmation: ProactiveConfirmation::Disabled,
            };
        }
    };

    let mut decision = parse_decision_json(&raw);
    // Models sometimes invent screen_digest from prompt examples when no screen
    // context was provided — clear it fail-closed.
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

fn truncate_history(history: &[HistoryEntry], max_chars: usize) -> Vec<HistoryEntry> {
    if max_chars == 0 || history.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    for entry in history.iter().rev() {
        let content = truncate_chars(&entry.content, max_chars.saturating_sub(used).max(1));
        let cost = content.chars().count().saturating_add(8);
        if used.saturating_add(cost) > max_chars && !out.is_empty() {
            break;
        }
        used = used.saturating_add(cost);
        out.push(HistoryEntry {
            role: entry.role,
            content,
        });
    }
    out.reverse();
    out
}

pub(crate) fn truncate_chars(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

fn redact_window_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len().min(200));
    for token in label.split_whitespace() {
        if token.contains('@') || token.contains('/') || token.contains('\\') {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    if out.is_empty() {
        "window".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProactiveSourcesConfig;
    use async_trait::async_trait;
    use ene_ai::{LlmCompletion, LlmMessage, LlmProviderError, LlmResponseChunk, Role};
    use std::pin::Pin;
    use tokio_stream::Stream;

    struct FixedProvider {
        body: String,
    }

    struct SchemaCaptureProvider {
        body: String,
        captured: std::sync::Mutex<Option<serde_json::Value>>,
    }

    #[async_trait]
    impl LlmProvider for FixedProvider {
        fn name(&self) -> &'static str {
            "fixed"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            Err(LlmProviderError::Provider("stream unused".into()))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, LlmProviderError> {
            Ok(LlmCompletion::text_only(self.body.clone()))
        }
    }

    #[async_trait]
    impl LlmProvider for SchemaCaptureProvider {
        fn name(&self) -> &'static str {
            "schema-capture"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
            LlmProviderError,
        > {
            Err(LlmProviderError::Provider("stream unused".into()))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, LlmProviderError> {
            if let Ok(mut guard) = self.captured.lock() {
                *guard = json_schema;
            }
            Ok(LlmCompletion::text_only(self.body.clone()))
        }
    }

    fn base_config() -> ProactiveConfig {
        ProactiveConfig {
            enabled: true,
            min_idle_seconds: 30,
            cooldown_seconds: 60,
            max_turns_per_session: 3,
            ..ProactiveConfig::default()
        }
    }

    fn memory_item(
        character: &str,
        user: &str,
        kind: ene_core::MemoryKind,
        title: &str,
        content: &str,
        status: ene_core::MemoryStatus,
    ) -> ene_core::NewMemoryItem {
        use ene_core::{
            AffectAnnotation, MemoryConfidence, MemorySalience, MemoryScope, MemorySource,
        };
        ene_core::NewMemoryItem {
            scope: if user.is_empty() {
                MemoryScope::Character
            } else {
                MemoryScope::User
            },
            character_id: character.into(),
            user_id: user.into(),
            kind,
            title: title.into(),
            content: content.into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.9),
            salience: MemorySalience::new(0.9),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        }
    }

    #[tokio::test]
    async fn memory_notes_load_only_the_users_standing_rules() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;
        use ene_core::MemoryKind;

        let port = InMemoryMemoryPort::default();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "Do not disturb",
            "don't talk while I work",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::UserProfile,
            "Night owl",
            "quiet at night",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        // Character-level row without a user id still counts for the user.
        port.insert_typed_memory(&memory_item(
            "ene",
            "",
            MemoryKind::Preference,
            "House rule",
            "no singing in the office",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        // Wrong user, wrong kind, and archived rows must never surface.
        port.insert_typed_memory(&memory_item(
            "ene",
            "bob",
            MemoryKind::Preference,
            "Bob rule",
            "loud music only",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Semantic,
            "Fact",
            "the sky is blue",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "Old rule",
            "call me in the morning",
            MemoryStatus::Archived,
        ))
        .await
        .unwrap();

        let notes = load_proactive_memory_notes(&port, "ene", "alice", 12)
            .await
            .expect("load memory notes");
        assert_eq!(notes.len(), 3);
        assert!(notes.iter().any(|n| n.contains("don't talk while I work")));
        assert!(notes.iter().any(|n| n.contains("quiet at night")));
        assert!(notes.iter().any(|n| n.contains("no singing in the office")));
        assert!(!notes.iter().any(|n| n.contains("loud music")));
        assert!(!notes.iter().any(|n| n.contains("sky is blue")));
        assert!(!notes.iter().any(|n| n.contains("morning")));
    }

    #[tokio::test]
    async fn memory_notes_respect_the_cap() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;
        use ene_core::MemoryKind;

        let port = InMemoryMemoryPort::default();
        for i in 0..4i64 {
            port.insert_typed_memory(&memory_item(
                "ene",
                "alice",
                MemoryKind::Preference,
                &format!("rule {i}"),
                "some standing rule",
                MemoryStatus::Active,
            ))
            .await
            .unwrap();
        }
        let notes = load_proactive_memory_notes(&port, "ene", "alice", 2)
            .await
            .expect("load memory notes");
        assert_eq!(notes.len(), 2);
        assert!(
            load_proactive_memory_notes(&port, "ene", "alice", 0)
                .await
                .expect("zero cap is a no-op")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn memory_notes_survive_fetch_window_competition() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;
        use ene_core::MemoryKind;

        let port = InMemoryMemoryPort::default();
        // Rows that do not apply to the user (other users, archived) sit at
        // the front of the fetch window; the user's standing rule must still
        // surface instead of being displaced out of it.
        port.insert_typed_memory(&memory_item(
            "ene",
            "bob",
            MemoryKind::Preference,
            "Bob rule",
            "loud music only",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "bob",
            MemoryKind::Preference,
            "Bob rule 2",
            "no singing",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "Stale rule",
            "archived so irrelevant",
            MemoryStatus::Archived,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "Old rule",
            "call me in the morning",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();

        let notes = load_proactive_memory_notes(&port, "ene", "alice", 1)
            .await
            .expect("load memory notes");
        assert_eq!(notes.len(), 1);
        assert!(notes.iter().any(|n| n.contains("call me in the morning")));
    }

    #[tokio::test]
    async fn memory_notes_skip_empty_rows() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;
        use ene_core::MemoryKind;

        let port = InMemoryMemoryPort::default();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "Real rule",
            "quiet at night",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();
        port.insert_typed_memory(&memory_item(
            "ene",
            "alice",
            MemoryKind::Preference,
            "",
            "",
            MemoryStatus::Active,
        ))
        .await
        .unwrap();

        let notes = load_proactive_memory_notes(&port, "ene", "alice", 12)
            .await
            .expect("load memory notes");
        assert_eq!(notes.len(), 1);
        assert!(notes.iter().all(|n| n.contains("quiet at night")));
    }

    #[test]
    fn disabled_sources_are_omitted_from_context() {
        let mut config = base_config();
        config.sources = ProactiveSourcesConfig {
            conversation: false,
            activity: false,
            screen_summary: false,
            memory: false,
            window_title_level: crate::config::WindowTitleLevel::AppOnly,
        };
        let history = vec![HistoryEntry {
            role: Role::User,
            content: "hello".into(),
        }];
        let observation = ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(99),
                active_window_label: "Editor".into(),
                recent_change: "focus".into(),
            }),
            screen_summary: Some("secret".into()),
            screen_summary_status: ScreenSummaryStatus::Available,
        };
        let ctx = build_proactive_context(
            &config,
            &history,
            &observation,
            None,
            &[],
            &[],
            ProactiveSuppressionState {
                seconds_since_user_input: 120,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            QuietHoursEval::inactive(),
            None,
            None,
        );
        assert!(ctx.history.is_empty());
        assert!(ctx.activity.is_none());
        assert!(ctx.screen_summary.is_none());
    }

    #[test]
    fn world_state_summary_reaches_context_only_when_enabled() {
        use crate::config::WorldStateConfig;

        let mut memory = WorldStateMemory::default();
        let world_cfg = WorldStateConfig {
            enabled: true,
            ..WorldStateConfig::default()
        };
        for i in 0..3 {
            let observation = ProactiveObservation {
                captured_at_unix_ms: i + 1,
                activity: Some(ActivitySnapshot {
                    idle_seconds: Some(10 * (i + 1)),
                    active_window_label: "Code".into(),
                    recent_change: String::new(),
                }),
                screen_summary: None,
                screen_summary_status: ScreenSummaryStatus::Disabled,
            };
            memory.push(
                WorldStateSnapshot::from_observation(&observation, 60),
                &world_cfg,
            );
        }
        let mut config = base_config();
        config.world_state = world_cfg.clone();
        let ctx = build_proactive_context(
            &config,
            &[],
            &ProactiveObservation::default(),
            None,
            &[],
            &[],
            ProactiveSuppressionState {
                seconds_since_user_input: 60,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            QuietHoursEval::inactive(),
            None,
            Some(&memory),
        );
        let summary = ctx
            .world_state
            .as_ref()
            .expect("enabled world state must produce a summary");
        assert_eq!(summary.idle_trend, IdleTrend::Rising);
        assert_eq!(summary.snapshot_count, 3);

        // The feature defaults to off: the same memory must not reach the
        // context, so the prompt and gates are byte-identical to before.
        let config = base_config();
        let ctx = build_proactive_context(
            &config,
            &[],
            &ProactiveObservation::default(),
            None,
            &[],
            &[],
            ProactiveSuppressionState {
                seconds_since_user_input: 60,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            QuietHoursEval::inactive(),
            None,
            Some(&memory),
        );
        assert!(ctx.world_state.is_none());
    }

    #[tokio::test]
    async fn gate_rejects_without_calling_llm() {
        let config = base_config();
        let ctx = ProactiveContext {
            history: vec![],
            seconds_since_user_input: 5,
            activity: None,
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 5,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":1.0}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider), "en").await;
        assert!(!outcome.llm_invoked);
        assert!(!outcome.decision.should_speak);
        assert!(matches!(
            outcome.skip,
            Some(ProactiveSkipReason::Gate(GateRejectReason::MinIdle))
        ));
    }

    #[tokio::test]
    async fn high_fatigue_suppresses_before_calling_llm() {
        let config = base_config();
        let ctx = ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hi".into(),
            }],
            seconds_since_user_input: 200,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            affect_summary: Some("valence=0.10 arousal=0.10 dominance=0.10 fatigue=0.85".into()),
            fatigue: Some(0.85),
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":1.0}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider), "en").await;
        assert!(!outcome.llm_invoked);
        assert!(!outcome.decision.should_speak);
        assert_eq!(
            outcome.skip,
            Some(ProactiveSkipReason::Gate(GateRejectReason::HighFatigue))
        );
    }

    #[tokio::test]
    async fn parse_success_allows_generation() {
        let config = base_config();
        let ctx = ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hey".into(),
            }],
            seconds_since_user_input: 200,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider), "en").await;
        assert!(outcome.llm_invoked);
        assert!(
            outcome
                .decision
                .allows_generation(config.decision.min_confidence)
        );
        assert_eq!(outcome.decision.topic_hint, "check in");
        assert_eq!(outcome.decision.urgency, ProactiveUrgency::Low);
        assert!(outcome.skip.is_none());
    }

    #[tokio::test]
    async fn decision_passes_inner_json_schema_object() {
        let config = base_config();
        let ctx = ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hey".into(),
            }],
            seconds_since_user_input: 200,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        };
        let capture = Arc::new(SchemaCaptureProvider {
            body: r#"{"should_speak":false,"confidence":0.1,"reason":"quiet","topic_hint":"","urgency":"low"}"#.into(),
            captured: std::sync::Mutex::new(None),
        });
        let provider: Arc<dyn LlmProvider> = capture.clone();
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider), "en").await;
        assert!(outcome.llm_invoked);
        assert_eq!(
            outcome.skip,
            Some(ProactiveSkipReason::ModelDeclined),
            "should_speak=false must be reported as ModelDeclined"
        );
        assert_eq!(outcome.decision.reason, "quiet");
        let schema = capture
            .captured
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .expect("schema passed to decision provider");
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
        assert!(schema.get("properties").is_some());
        assert!(schema.get("schema").is_none());
    }

    #[tokio::test]
    async fn model_decline_reports_model_declined_skip() {
        let config = base_config();
        let ctx = ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hey".into(),
            }],
            seconds_since_user_input: 200,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":false,"confidence":0.9,"reason":"quiet","topic_hint":"","urgency":"low"}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider), "en").await;
        assert!(outcome.llm_invoked);
        assert!(
            !outcome
                .decision
                .allows_generation(config.decision.min_confidence),
            "should_speak=false blocks generation even at high confidence"
        );
        assert_eq!(
            outcome.skip,
            Some(ProactiveSkipReason::ModelDeclined),
            "a model decline must be distinguishable from speaking (None)"
        );
    }

    #[test]
    fn model_declined_display_matches_runtime_fallback() {
        assert_eq!(
            ProactiveSkipReason::ModelDeclined.to_string(),
            "model declined"
        );
    }

    fn speak_context() -> ProactiveContext {
        ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hey".into(),
            }],
            seconds_since_user_input: 200,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(200),
                active_window_label: "Browser".into(),
                recent_change: String::new(),
            }),
            screen_summary: None,
            affect_summary: None,
            fatigue: None,
            commitments: vec![],
            user_instructions: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            quiet_hours: QuietHoursEval::inactive(),
            pending_confirmation: None,
            world_state: None,
        }
    }

    #[tokio::test]
    async fn confirmation_is_disabled_by_default() {
        let config = base_config();
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome =
            decide_proactive_speech(&config, &speak_context(), Some(provider), "en").await;
        assert!(outcome.skip.is_none());
        assert_eq!(outcome.confirmation, ProactiveConfirmation::Disabled);
    }

    #[tokio::test]
    async fn confirmation_is_pending_when_enabled_and_generation_starts() {
        let mut config = base_config();
        config.confirmation_enabled = true;
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome =
            decide_proactive_speech(&config, &speak_context(), Some(provider), "en").await;
        assert!(outcome.skip.is_none());
        assert_eq!(outcome.confirmation, ProactiveConfirmation::Pending);
    }

    #[tokio::test]
    async fn confirmation_enabled_passes_borderline_confidence_to_main_model() {
        let mut config = base_config();
        config.confirmation_enabled = true;
        // 0.50 is below the configured 0.55 gate but above the effective 0.40
        // gate; the borderline case must reach the main model, not be dropped.
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.5,"reason":"unsure","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome =
            decide_proactive_speech(&config, &speak_context(), Some(provider), "en").await;
        assert!(outcome.skip.is_none());
        assert!(
            outcome
                .decision
                .allows_generation(config.effective_decision_min_confidence())
        );
        assert_eq!(outcome.confirmation, ProactiveConfirmation::Pending);
    }

    #[tokio::test]
    async fn confirmation_disabled_rejects_borderline_confidence() {
        let config = base_config();
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.5,"reason":"unsure","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome =
            decide_proactive_speech(&config, &speak_context(), Some(provider), "en").await;
        assert_eq!(outcome.skip, Some(ProactiveSkipReason::BelowConfidence));
        assert_eq!(outcome.confirmation, ProactiveConfirmation::Disabled);
    }

    #[test]
    fn confirmation_declined_display_matches_runtime_detail() {
        assert_eq!(
            ProactiveSkipReason::ConfirmationDeclined.to_string(),
            "confirmation declined"
        );
    }

    #[test]
    fn confirmation_display_uses_stable_english_labels() {
        assert_eq!(ProactiveConfirmation::Disabled.to_string(), "disabled");
        assert_eq!(ProactiveConfirmation::Pending.to_string(), "pending");
        assert_eq!(ProactiveConfirmation::Accepted.to_string(), "accepted");
        assert_eq!(ProactiveConfirmation::Declined.to_string(), "declined");
        assert_eq!(ProactiveConfirmation::Empty.to_string(), "empty");
    }

    #[test]
    fn silent_token_is_pinned_and_named_in_prompt_notes() {
        assert_eq!(SILENT_TOKEN, "<|silent|>");
        for lang in ["en", "ja"] {
            let library = ene_config::PromptLibrary::load(lang);
            let note = &library.proactive().confirmation_note;
            assert!(
                note.contains(SILENT_TOKEN),
                "{lang} confirmation_note must name the refusal token"
            );
        }
    }
}
