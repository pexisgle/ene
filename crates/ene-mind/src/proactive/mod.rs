//! Proactive companion speech decision pipeline (#103 / #164).
//!
//! Builds a normalized [`ProactiveContext`], applies deterministic gates, asks a
//! lightweight LLM for structured JSON only, and fail-closes on any error.

mod gate;
mod parse;
mod prompt;

pub use gate::{GateRejectReason, evaluate_deterministic_gates};
pub use parse::{decision_schema, decision_schema_object, parse_decision_json};
pub use prompt::build_decision_messages;

use std::sync::Arc;
use std::time::Duration;

use ene_ai::LlmProvider;
use ene_store::{ActiveCommitmentPrompt, AffectState};
use serde::{Deserialize, Serialize};

use crate::config::ProactiveConfig;
use crate::lifecycle::HistoryEntry;

/// Privacy-safe activity snapshot from the host (desktop).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    /// Seconds since the last OS activity signal when available.
    pub idle_seconds: Option<u64>,
    /// Privacy-safe app label (never a raw window title).
    pub active_window_label: String,
    /// Short description of recent activity change (optional).
    pub recent_change: String,
}

/// Screen summary availability from the host (#168).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSummaryStatus {
    /// Source disabled in settings.
    #[default]
    Disabled,
    /// Source enabled but no summarizer is available on this host.
    Unavailable,
    /// Short-lived text summary (never raw image bytes).
    Available,
}

/// Host-supplied observation used to build decision context (#166 / #168).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProactiveObservation {
    /// When this observation was captured (unix millis).
    pub captured_at_unix_ms: u64,
    /// Activity signals when the activity source is enabled.
    pub activity: Option<ActivitySnapshot>,
    /// Short-lived screen **text** summary when [`ScreenSummaryStatus::Available`].
    pub screen_summary: Option<String>,
    /// Whether screen summary was requested but unavailable.
    pub screen_summary_status: ScreenSummaryStatus,
}

/// Runtime suppression counters passed into the decision pipeline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProactiveSuppressionState {
    /// Seconds since the last user message in the session.
    pub seconds_since_user_input: u64,
    /// Seconds since the last successful proactive utterance (or `u64::MAX` if none).
    pub seconds_since_proactive: u64,
    /// Proactive utterances already completed in this session.
    pub proactive_turns_this_session: usize,
    /// True while a user turn, tool call, or permission/input wait is active.
    pub user_turn_busy: bool,
}

/// Normalized input for a proactive decision.
#[derive(Debug, Clone)]
pub struct ProactiveContext {
    /// Recent history entries already truncated for the prompt budget.
    pub history: Vec<HistoryEntry>,
    /// Seconds since last user input.
    pub seconds_since_user_input: u64,
    /// Activity snapshot when the source is enabled and available.
    pub activity: Option<ActivitySnapshot>,
    /// Screen summary when the source is enabled and available.
    pub screen_summary: Option<String>,
    /// Current affect summary line for the prompt (optional).
    pub affect_summary: Option<String>,
    /// Active commitment one-liners (optional).
    pub commitments: Vec<String>,
    /// Suppression counters at decision time.
    pub suppression: ProactiveSuppressionState,
}

/// Urgency hint from the decision model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProactiveUrgency {
    /// Low priority.
    Low,
    /// Default priority.
    #[default]
    Normal,
    /// High priority.
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

/// Structured decision returned by the lightweight model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProactiveDecision {
    /// Whether generation should start.
    pub should_speak: bool,
    /// Model confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Internal reason (diagnostics only; never spoken verbatim).
    pub reason: String,
    /// Optional topic hint for generation.
    pub topic_hint: String,
    /// Urgency hint.
    pub urgency: ProactiveUrgency,
}

impl ProactiveDecision {
    /// Fail-closed "do not speak" decision.
    #[must_use]
    pub fn silent(reason: impl Into<String>) -> Self {
        Self {
            should_speak: false,
            confidence: 0.0,
            reason: reason.into(),
            topic_hint: String::new(),
            urgency: ProactiveUrgency::Normal,
        }
    }

    /// True when generation should proceed given the configured threshold.
    #[must_use]
    pub fn allows_generation(&self, min_confidence: f64) -> bool {
        self.should_speak && self.confidence >= min_confidence
    }
}

/// Why a proactive decision was skipped or silenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSkipReason {
    /// Feature disabled in config.
    Disabled,
    /// Deterministic gate rejected before calling the LLM.
    Gate(GateRejectReason),
    /// Decision model / parse / timeout failed (fail-closed).
    DecisionFailed(String),
    /// Model returned `should_speak` but confidence was too low.
    BelowConfidence,
}

/// Outcome of [`decide_proactive_speech`].
#[derive(Debug, Clone)]
pub struct ProactiveDecisionOutcome {
    /// Normalized decision (always present; fail-closed on errors).
    pub decision: ProactiveDecision,
    /// Optional skip / silence reason for diagnostics.
    pub skip: Option<ProactiveSkipReason>,
    /// True when the lightweight LLM was actually invoked.
    pub llm_invoked: bool,
}

/// Build a [`ProactiveContext`] from session history + host observation.
#[must_use]
pub fn build_proactive_context(
    config: &ProactiveConfig,
    history: &[HistoryEntry],
    observation: &ProactiveObservation,
    affect: Option<&AffectState>,
    commitments: &[ActiveCommitmentPrompt],
    suppression: ProactiveSuppressionState,
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
            "valence={:.2} arousal={:.2} dominance={:.2}",
            a.valence, a.arousal, a.dominance
        )
    });

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

    ProactiveContext {
        history,
        seconds_since_user_input: suppression.seconds_since_user_input,
        activity,
        screen_summary,
        affect_summary,
        commitments,
        suppression,
    }
}

/// Run deterministic gates + optional LLM decision. Always fail-closed.
pub async fn decide_proactive_speech(
    config: &ProactiveConfig,
    context: &ProactiveContext,
    provider: Option<Arc<dyn LlmProvider>>,
) -> ProactiveDecisionOutcome {
    if !config.enabled {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent("proactive disabled"),
            skip: Some(ProactiveSkipReason::Disabled),
            llm_invoked: false,
        };
    }

    if let Err(reason) = evaluate_deterministic_gates(config, context) {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent(reason.to_string()),
            skip: Some(ProactiveSkipReason::Gate(reason)),
            llm_invoked: false,
        };
    }

    let Some(provider) = provider else {
        return ProactiveDecisionOutcome {
            decision: ProactiveDecision::silent("decision provider unavailable"),
            skip: Some(ProactiveSkipReason::DecisionFailed(
                "decision provider unavailable".to_string(),
            )),
            llm_invoked: false,
        };
    };

    let messages = build_decision_messages(context);
    let schema = decision_schema();
    let timeout = Duration::from_secs(config.decision_timeout_seconds.max(1));

    let raw = match tokio::time::timeout(timeout, provider.chat_completion(&messages, Some(schema)))
        .await
    {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            return ProactiveDecisionOutcome {
                decision: ProactiveDecision::silent(format!("decision provider error: {e}")),
                skip: Some(ProactiveSkipReason::DecisionFailed(e.to_string())),
                llm_invoked: true,
            };
        }
        Err(_) => {
            return ProactiveDecisionOutcome {
                decision: ProactiveDecision::silent("decision timed out"),
                skip: Some(ProactiveSkipReason::DecisionFailed(
                    "decision timed out".to_string(),
                )),
                llm_invoked: true,
            };
        }
    };

    let decision = parse_decision_json(&raw);
    if !decision.allows_generation(config.decision.min_confidence) {
        let skip = if decision.should_speak {
            Some(ProactiveSkipReason::BelowConfidence)
        } else {
            None
        };
        return ProactiveDecisionOutcome {
            decision,
            skip,
            llm_invoked: true,
        };
    }

    ProactiveDecisionOutcome {
        decision,
        skip: None,
        llm_invoked: true,
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

fn truncate_chars(input: &str, max_chars: usize) -> String {
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
    // Drop obvious path-like and email-like fragments from window titles.
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
    use ene_ai::{LlmMessage, LlmProviderError, LlmResponseChunk, Role};
    use std::pin::Pin;
    use tokio_stream::Stream;

    struct FixedProvider {
        body: String,
    }

    #[async_trait]
    impl LlmProvider for FixedProvider {
        fn name(&self) -> &'static str {
            "fixed"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
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
        ) -> Result<String, LlmProviderError> {
            Ok(self.body.clone())
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

    #[test]
    fn disabled_sources_are_omitted_from_context() {
        let mut config = base_config();
        config.sources = ProactiveSourcesConfig {
            conversation: false,
            activity: false,
            screen_summary: false,
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
            ProactiveSuppressionState {
                seconds_since_user_input: 120,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
        );
        assert!(ctx.history.is_empty());
        assert!(ctx.activity.is_none());
        assert!(ctx.screen_summary.is_none());
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
            commitments: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 5,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":1.0}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider)).await;
        assert!(!outcome.llm_invoked);
        assert!(!outcome.decision.should_speak);
        assert!(matches!(
            outcome.skip,
            Some(ProactiveSkipReason::Gate(GateRejectReason::MinIdle))
        ));
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
            commitments: vec![],
            suppression: ProactiveSuppressionState {
                seconds_since_user_input: 200,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
        };
        let provider: Arc<dyn LlmProvider> = Arc::new(FixedProvider {
            body: r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"check in","urgency":"low"}"#.into(),
        });
        let outcome = decide_proactive_speech(&config, &ctx, Some(provider)).await;
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
}
