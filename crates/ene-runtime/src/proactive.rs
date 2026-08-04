use ene_ai::LlmProvider;
use ene_mind::{
    ActiveCommitmentPrompt, PendingConfirmationPrompt, PendingResolutionVerdict, ProactiveConfig,
    ProactiveConfirmation, ProactiveObservation, ProactiveSkipReason, ProactiveSuppressionState,
    QuietHoursEval, ScreenSummaryStatus, WorldStateMemory, build_proactive_context,
    build_resolution_messages, decide_proactive_speech, parse_resolution_json,
    resolution_schema_object,
};
use ene_store::AffectState as StoreAffectState;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::TerminalReason;
use crate::types::TurnId;

/// Mutable scheduler counters owned by the actor.
#[derive(Debug)]
pub(crate) struct ProactiveScheduler {
    pub observation: ProactiveObservation,
    /// Ephemeral screen frame from the last successful vision summarize (data URI).
    /// Never persisted; used only for the next proactive generation when the
    /// generation model declares `supports_vision`.
    pub last_screen_image_data_uri: Option<String>,
    pub last_user_input_at: Instant,
    pub last_proactive_at: Option<Instant>,
    pub proactive_turns: usize,
    /// Bumped whenever a user turn starts so in-flight decisions are discarded.
    pub epoch: u64,
    /// Bounded in-memory ring of world-state snapshots (never persisted).
    pub world_state: WorldStateMemory,
    /// Decision behind the proactive generation currently in flight, kept so
    /// the stream-completion path can log decision/main-model agreement.
    pub last_decision: Option<ProactiveDecisionResult>,
    /// Suppressed quiet-hours moments awaiting catch-up delivery (times only,
    /// never screen data). Bounded; oldest entries are dropped first.
    pub(crate) quiet_hours_queue: VecDeque<QueuedQuietHour>,
    /// Candidate whose confirmation question was delivered (visible speech)
    /// and whose reply is still expected. Blocks re-selection so only one
    /// confirmation is in flight; consumed when the reply is classified.
    pub(crate) asked_pending_candidate: Option<PendingConfirmationPrompt>,
    /// When each candidate's confirmation question was last delivered, for
    /// per-candidate re-ask backoff. Survives the asked-marker consumption
    /// so an unclear reply cannot re-arm the same question immediately.
    pub(crate) pending_confirmation_asked_at: HashMap<i64, chrono::DateTime<chrono::Utc>>,
    /// Bumped on session reset only, so reply classifications stay valid
    /// across user turns (which bump [`Self::epoch`]) and go stale only when
    /// the character/session actually changed.
    pub(crate) session_epoch: u64,
}

impl Default for ProactiveScheduler {
    fn default() -> Self {
        Self {
            observation: ProactiveObservation::default(),
            last_screen_image_data_uri: None,
            last_user_input_at: Instant::now(),
            last_proactive_at: None,
            proactive_turns: 0,
            epoch: 0,
            world_state: WorldStateMemory::default(),
            last_decision: None,
            quiet_hours_queue: VecDeque::new(),
            asked_pending_candidate: None,
            pending_confirmation_asked_at: HashMap::new(),
            session_epoch: 0,
        }
    }
}

/// One quiet-hours-suppressed proactive moment, queued for catch-up delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedQuietHour {
    /// Local date at the moment (`YYYY-MM-DD`); disambiguates multi-night
    /// queues.
    pub local_date: String,
    /// Local wall time at the moment (`HH:MM`).
    pub local_time: String,
}

/// Maximum queued quiet-hours moments kept for catch-up delivery.
pub(crate) const QUIET_HOURS_QUEUE_CAP: usize = 32;

/// Bounds the in-process fallback for pending-confirmation delivery timestamps.
/// Resolved candidates are removed normally; this cap covers rows that expire
/// or are resolved through another API while the actor remains alive.
const PENDING_CONFIRMATION_BACKOFF_CAP: usize = 256;

/// Maximum suppressed moments rendered into one summary catch-up hint.
pub(crate) const QUIET_HOURS_SUMMARY_MAX_ITEMS: usize = 20;

impl ProactiveScheduler {
    /// Record that a user turn began (cancels stale decisions via epoch bump
    /// and drops queued quiet-hours moments — the user is back at the desk).
    pub fn on_user_turn_started(&mut self) {
        self.last_user_input_at = Instant::now();
        self.epoch = self.epoch.wrapping_add(1);
        if !self.quiet_hours_queue.is_empty() {
            tracing::debug!(
                component = "Proactive",
                dropped = self.quiet_hours_queue.len(),
                "User turn discards the pending quiet-hours catch-up queue"
            );
            self.quiet_hours_queue.clear();
        }
    }

    pub fn on_proactive_completed(&mut self) {
        self.last_proactive_at = Some(Instant::now());
        self.proactive_turns = self.proactive_turns.saturating_add(1);
    }

    /// Record a main-model decline: apply the cooldown so the next tick does
    /// not immediately re-run generation, without consuming the per-session
    /// utterance budget.
    pub fn on_proactive_declined(&mut self) {
        self.last_proactive_at = Some(Instant::now());
    }

    /// Reset per-session counters (character / session change).
    pub fn reset_session(&mut self) {
        self.proactive_turns = 0;
        self.last_proactive_at = None;
        self.epoch = self.epoch.wrapping_add(1);
        self.observation = ProactiveObservation::default();
        self.last_screen_image_data_uri = None;
        self.last_user_input_at = Instant::now();
        self.last_decision = None;
        self.quiet_hours_queue.clear();
        self.asked_pending_candidate = None;
        self.world_state.clear();
        // Keep delivery timestamps across session changes. A session reset
        // must not make an unclear candidate immediately eligible again.
        self.session_epoch = self.session_epoch.wrapping_add(1);
    }

    fn remember_pending_confirmation_delivery(&mut self, candidate_id: i64) {
        if !self
            .pending_confirmation_asked_at
            .contains_key(&candidate_id)
            && self.pending_confirmation_asked_at.len() >= PENDING_CONFIRMATION_BACKOFF_CAP
            && let Some((&oldest_id, _)) = self
                .pending_confirmation_asked_at
                .iter()
                .min_by_key(|(_, asked_at)| *asked_at)
        {
            self.pending_confirmation_asked_at.remove(&oldest_id);
        }
        self.pending_confirmation_asked_at
            .insert(candidate_id, chrono::Utc::now());
    }

    /// Take the stashed screen image for a generation turn (clears the stash).
    pub fn take_screen_image(&mut self) -> Option<String> {
        self.last_screen_image_data_uri.take()
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

/// Encode an RGB8 buffer to a JPEG data URI for OpenAI-compatible vision parts.
pub(crate) fn rgb_to_jpeg_data_uri(width: u32, height: u32, rgb: &[u8]) -> Result<String, String> {
    use base64::Engine as _;
    use image::ImageEncoder;

    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3);
    if rgb.len() != expected {
        return Err(format!(
            "rgb length mismatch (got {}, expected {expected})",
            rgb.len()
        ));
    }

    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
        .write_image(rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("jpeg encode failed: {e}"))?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

/// Result of an async decision task.
#[derive(Debug, Clone)]
pub(crate) struct ProactiveDecisionResult {
    pub epoch: u64,
    pub should_generate: bool,
    /// Model `should_speak` flag (before confidence gate).
    pub should_speak: bool,
    /// Model confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    pub llm_invoked: bool,
    pub topic_hint: String,
    pub detail: String,
    /// Main-model confirmation state (disabled / pending at decision time;
    /// the actor resolves it once the generation stream ends).
    pub confirmation: ene_mind::ProactiveConfirmation,
    /// True for synthetic quiet-hours catch-up results: no decision LLM ran
    /// and no stashed screen frame may attach (the catch-up note only knows
    /// that moments occurred, never their content).
    pub(crate) catch_up: bool,
    /// Due pending candidate the decision judged, if any. The actor renders
    /// the confirmation question from it instead of a topic hint.
    pub pending_confirmation: Option<PendingConfirmationPrompt>,
}

/// Drop stale activity/screen payloads so decisions do not act on old host signals.
#[must_use]
pub(crate) fn sanitize_observation(
    config: &ProactiveConfig,
    mut observation: ProactiveObservation,
) -> ProactiveObservation {
    if observation.captured_at_unix_ms == 0 {
        observation.activity = None;
        observation.screen_summary = None;
        observation.screen_summary_status = ScreenSummaryStatus::Unavailable;
        return observation;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    // Screen summaries must stay within one observe interval; activity may be slightly older.
    let max_age_secs = if config.sources.screen_summary {
        config.interval_seconds.max(1)
    } else {
        config.interval_seconds.saturating_mul(3).max(1)
    };
    let age_secs = now_ms
        .saturating_sub(observation.captured_at_unix_ms)
        .saturating_div(1000);
    if age_secs > max_age_secs {
        observation.activity = None;
        observation.screen_summary = None;
        if config.sources.screen_summary {
            observation.screen_summary_status = ScreenSummaryStatus::Unavailable;
        }
    }
    observation
}

/// Run a decision against the current session snapshot (spawned off the actor).
pub(crate) async fn run_decision_task(
    config: ProactiveConfig,
    history: Vec<ene_mind::HistoryEntry>,
    observation: ProactiveObservation,
    world_state: Option<WorldStateMemory>,
    suppression: ProactiveSuppressionState,
    quiet_hours: QuietHoursEval,
    pending_confirmation: Option<PendingConfirmationPrompt>,
    provider: Option<Arc<dyn LlmProvider>>,
    epoch: u64,
    affect: Option<StoreAffectState>,
    commitments: Vec<ActiveCommitmentPrompt>,
    user_instructions: Vec<String>,
    prompt_language: String,
) -> ProactiveDecisionResult {
    let observation = sanitize_observation(&config, observation);
    let context = build_proactive_context(
        &config,
        &history,
        &observation,
        affect.as_ref(),
        &commitments,
        &user_instructions,
        suppression,
        quiet_hours,
        pending_confirmation.clone(),
        world_state.as_ref(),
    );
    let outcome = decide_proactive_speech(&config, &context, provider, &prompt_language).await;
    let should_generate = outcome.skip.is_none()
        && outcome
            .decision
            .allows_generation(config.effective_decision_min_confidence());
    let detail = if let Some(skip) = &outcome.skip {
        skip.to_string()
    } else if outcome.decision.should_speak {
        if outcome.decision.reason.is_empty() {
            "will speak".to_string()
        } else {
            outcome.decision.reason.clone()
        }
    } else if outcome.decision.reason.is_empty() {
        "model declined".to_string()
    } else {
        outcome.decision.reason.clone()
    };
    ProactiveDecisionResult {
        epoch,
        should_generate,
        should_speak: outcome.decision.should_speak,
        confidence: outcome.decision.confidence,
        llm_invoked: outcome.llm_invoked,
        topic_hint: outcome.decision.topic_hint,
        detail,
        confirmation: outcome.confirmation,
        catch_up: false,
        pending_confirmation: pending_confirmation.clone(),
    }
}

/// Build the internal generation hint (never stored as a user message).
#[must_use]
pub(crate) fn proactive_generation_hint(
    topic_hint: &str,
    prompt_language: &str,
    confirmation_enabled: bool,
) -> String {
    let library = ene_config::PromptLibrary::load(prompt_language);
    let mut prompts = library.proactive().clone();
    if confirmation_enabled {
        ensure_confirmation_note(&mut prompts, prompt_language);
    }
    prompts.render_generation_hint(topic_hint, confirmation_enabled)
}

/// Ensure the `<|silent|>` refusal note is non-empty for a pack that ships
/// without one: fall back to the sibling locale, then to an embedded string,
/// so confirmation-enabled generations never lose the refusal instruction.
fn ensure_confirmation_note(
    prompts: &mut ene_config::prompts::ProactivePrompts,
    prompt_language: &str,
) {
    if !prompts.confirmation_note.trim().is_empty() {
        return;
    }
    let fallback_language = if ene_config::resolve_language_alias(prompt_language) == "ja" {
        "ja"
    } else {
        "en"
    };
    let fallback = ene_config::PromptLibrary::load(fallback_language)
        .proactive()
        .confirmation_note
        .clone();
    prompts.confirmation_note = if fallback.trim().is_empty() {
        "If you decide not to speak, emit exactly <|silent|> as the first token and nothing else."
            .to_string()
    } else {
        fallback
    };
    tracing::warn!(
        component = "Proactive",
        "confirmation_enabled requires a confirmation_note; using the embedded fallback"
    );
}

/// Render the generation hint for a pending-candidate confirmation question.
#[must_use]
pub(crate) fn proactive_pending_confirmation_hint(
    candidate: &PendingConfirmationPrompt,
    prompt_language: &str,
    confirmation_enabled: bool,
) -> String {
    let library = ene_config::PromptLibrary::load(prompt_language);
    let mut prompts = library.proactive().clone();
    if confirmation_enabled {
        ensure_confirmation_note(&mut prompts, prompt_language);
    }
    if prompts.pending_confirmation_note.trim().is_empty() {
        let fallback_language = if ene_config::resolve_language_alias(prompt_language) == "ja" {
            "ja"
        } else {
            "en"
        };
        let fallback = ene_config::PromptLibrary::load(fallback_language)
            .proactive()
            .pending_confirmation_note
            .clone();
        prompts.pending_confirmation_note = if fallback.trim().is_empty() {
            "Ask a short, natural question to confirm this unconfirmed memory candidate:\n{candidate}"
                .to_string()
        } else {
            fallback
        };
        tracing::warn!(
            component = "Proactive",
            "pending_confirmation_note is empty; using the embedded fallback"
        );
    }
    let title = candidate.title.chars().take(160).collect::<String>();
    let content = candidate.content.chars().take(400).collect::<String>();
    prompts.render_pending_confirmation_hint(&format!("{title}: {content}"), confirmation_enabled)
}

/// Render the catch-up generation hint for quiet-hours-suppressed moments.
#[must_use]
pub(crate) fn quiet_hours_catch_up_hint(items: &str, prompt_language: &str) -> String {
    let library = ene_config::PromptLibrary::load(prompt_language);
    let mut prompts = library.proactive().clone();
    if prompts.catch_up_note.trim().is_empty() {
        let fallback_language = if ene_config::resolve_language_alias(prompt_language) == "ja" {
            "ja"
        } else {
            "en"
        };
        let fallback = ene_config::PromptLibrary::load(fallback_language)
            .proactive()
            .catch_up_note
            .clone();
        prompts.catch_up_note = if fallback.trim().is_empty() {
            "Several moments passed while you were away. If it feels natural, \
             acknowledge them briefly: {items}"
                .to_string()
        } else {
            fallback
        };
        tracing::warn!(
            component = "Proactive",
            "catch_up_note is empty; using the embedded fallback"
        );
    }
    prompts.render_catch_up_hint(items)
}

/// Format queued quiet-hours moments as a compact, language-neutral item list
/// (`"2026-08-03 22:30, 2026-08-03 22:45"`).
#[must_use]
pub(crate) fn quiet_hours_items(entries: &[QueuedQuietHour]) -> String {
    entries
        .iter()
        .map(|entry| format!("{} {}", entry.local_date, entry.local_time))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether the proactive turn's status announcement is suppressed.
#[must_use]
pub(crate) fn quiet_hours_suppresses_notifications(
    quiet: &QuietHoursEval,
    suppress: ene_mind::QuietHoursSuppressConfig,
) -> bool {
    quiet.active && suppress.notifications
}

/// Whether proactive TTS audio is suppressed.
#[must_use]
pub(crate) fn quiet_hours_suppresses_tts(
    quiet: &QuietHoursEval,
    suppress: ene_mind::QuietHoursSuppressConfig,
) -> bool {
    quiet.active && suppress.tts
}

/// Resolve the final confirmation verdict from the terminal reason and
/// whether the turn streamed visible text.
///
/// A `Done` turn that produced no visible text (empty or marker-only
/// response) is not an acceptance: the model neither declined nor spoke.
#[must_use]
pub(crate) fn resolve_confirmation(
    terminal: &TerminalReason,
    decision_confirmation: ProactiveConfirmation,
    spoke_visible_text: bool,
) -> ProactiveConfirmation {
    match terminal {
        TerminalReason::Declined => ProactiveConfirmation::Declined,
        TerminalReason::Done if decision_confirmation == ProactiveConfirmation::Pending => {
            if spoke_visible_text {
                ProactiveConfirmation::Accepted
            } else {
                ProactiveConfirmation::Empty
            }
        }
        _ => decision_confirmation,
    }
}

/// Apply a proactive generation's terminal outcome to the scheduler and
/// resolve the confirmation verdict.
///
/// `Done` completes the turn (utterance budget consumed); `Declined` applies
/// the cooldown without consuming the budget.
#[must_use]
pub(crate) fn apply_proactive_completion(
    scheduler: &mut ProactiveScheduler,
    decision: &ProactiveDecisionResult,
    terminal: &TerminalReason,
    spoke_visible_text: bool,
) -> ProactiveConfirmation {
    let confirmation = resolve_confirmation(terminal, decision.confirmation, spoke_visible_text);
    if let Some(candidate) = &decision.pending_confirmation {
        // The asked marker tracks *delivery*, independent of the
        // main-model-confirmation feature: with `confirmation_enabled` off
        // the verdict stays `Disabled` even though the question was spoken.
        if matches!(terminal, TerminalReason::Done) && spoke_visible_text {
            scheduler.asked_pending_candidate = Some(candidate.clone());
            scheduler.remember_pending_confirmation_delivery(candidate.id);
        } else {
            scheduler.asked_pending_candidate = None;
        }
    }
    match terminal {
        TerminalReason::Done => scheduler.on_proactive_completed(),
        TerminalReason::Declined => scheduler.on_proactive_declined(),
        _ => {}
    }
    confirmation
}

/// Result of the reply-classification task for an asked candidate.
#[derive(Debug, Clone)]
pub(crate) struct PendingResolutionResult {
    /// [`ProactiveScheduler::session_epoch`] at spawn; results from a
    /// previous character/session are dropped.
    pub session_epoch: u64,
    /// `pending_candidates` row the verdict applies to.
    pub candidate_id: i64,
    /// Turn whose user message was classified; reported on
    /// `CandidateChanged`.
    pub turn: TurnId,
    pub verdict: PendingResolutionVerdict,
    pub detail: String,
}

/// Classify a user reply to a pending-candidate confirmation question
/// (spawned off the actor). Fail-closed: any provider/parse failure yields
/// [`PendingResolutionVerdict::Unclear`], which leaves the candidate pending.
pub(crate) async fn run_resolution_task(
    candidate: PendingConfirmationPrompt,
    reply: String,
    turn: TurnId,
    session_epoch: u64,
    provider: Arc<dyn LlmProvider>,
    prompt_language: String,
    timeout: Duration,
) -> PendingResolutionResult {
    let messages = build_resolution_messages(&candidate, &reply, &prompt_language);
    let schema = resolution_schema_object();
    let raw = match tokio::time::timeout(timeout, provider.chat_completion(&messages, Some(schema)))
        .await
    {
        Ok(Ok(completion)) => completion.text,
        Ok(Err(error)) => {
            tracing::warn!(
                component = "Proactive",
                event = "pending_resolution",
                candidate_id = candidate.id,
                error = %error,
                "Pending-candidate reply classification failed; leaving the candidate pending"
            );
            return PendingResolutionResult {
                session_epoch,
                candidate_id: candidate.id,
                turn,
                verdict: PendingResolutionVerdict::Unclear,
                detail: error.to_string(),
            };
        }
        Err(_) => {
            tracing::warn!(
                component = "Proactive",
                event = "pending_resolution",
                candidate_id = candidate.id,
                "Pending-candidate reply classification timed out; leaving the candidate pending"
            );
            return PendingResolutionResult {
                session_epoch,
                candidate_id: candidate.id,
                turn,
                verdict: PendingResolutionVerdict::Unclear,
                detail: "timed out".to_string(),
            };
        }
    };
    let verdict = parse_resolution_json(&raw);
    PendingResolutionResult {
        session_epoch,
        candidate_id: candidate.id,
        turn,
        verdict,
        detail: raw.trim().chars().take(200).collect(),
    }
}

/// Emit the decision/main-model agreement line for a confirmed generation.
pub(crate) fn log_confirmation(
    decision: &ProactiveDecisionResult,
    confirmation: ProactiveConfirmation,
) {
    match confirmation {
        ProactiveConfirmation::Declined => {
            tracing::info!(
                component = "Proactive",
                event = "confirmation",
                decision_should_speak = decision.should_speak,
                decision_confidence = decision.confidence,
                decision_llm_invoked = decision.llm_invoked,
                confirmation = %confirmation,
                skip = %ProactiveSkipReason::ConfirmationDeclined,
                "Proactive main model declined"
            );
        }
        ProactiveConfirmation::Accepted => {
            tracing::info!(
                component = "Proactive",
                event = "confirmation",
                decision_should_speak = decision.should_speak,
                decision_confidence = decision.confidence,
                decision_llm_invoked = decision.llm_invoked,
                confirmation = %confirmation,
                "Proactive decision/main-model agreement"
            );
        }
        ProactiveConfirmation::Empty => {
            tracing::info!(
                component = "Proactive",
                event = "confirmation",
                decision_should_speak = decision.should_speak,
                decision_confidence = decision.confidence,
                decision_llm_invoked = decision.llm_invoked,
                confirmation = %confirmation,
                "Proactive main model produced no speech"
            );
        }
        _ => {}
    }
}

/// Interval duration from config (minimum 1s).
#[must_use]
pub(crate) fn tick_period(config: &ProactiveConfig) -> Duration {
    Duration::from_secs(config.interval_seconds.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_observation_clears_activity() {
        let config = ProactiveConfig {
            enabled: true,
            interval_seconds: 10,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };
        let obs = ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ene_mind::ActivitySnapshot {
                idle_seconds: None,
                active_window_label: "app".into(),
                recent_change: String::new(),
            }),
            ..ProactiveObservation::default()
        };
        let sanitized = sanitize_observation(&config, obs);
        assert!(sanitized.activity.is_none());
    }

    #[test]
    fn rgb_to_jpeg_data_uri_has_prefix() {
        let rgb = [255u8, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
        let uri = rgb_to_jpeg_data_uri(2, 2, &rgb).expect("encode");
        assert!(uri.starts_with("data:image/jpeg;base64,"));
        assert!(uri.len() > "data:image/jpeg;base64,".len());
    }

    struct CaptureProvider {
        captured: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for CaptureProvider {
        fn name(&self) -> &'static str {
            "capture"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                        > + Send,
                >,
            >,
            ene_ai::LlmProviderError,
        > {
            Err(ene_ai::LlmProviderError::Provider("stream unused".into()))
        }

        async fn chat_completion(
            &self,
            messages: &[ene_ai::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
            for message in messages {
                if let ene_ai::LlmMessage::User { parts } = message {
                    for part in parts {
                        if let ene_ai::UserMessagePart::Text { text } = part {
                            *self.captured.lock().expect("lock") = Some(text.clone());
                        }
                    }
                }
            }
            Ok(ene_ai::LlmCompletion::text_only(
                r#"{"should_speak":false,"confidence":0.9,"reason":"quiet","topic_hint":"","urgency":"low"}"#
                    .into(),
            ))
        }
    }

    #[tokio::test]
    async fn decision_context_carries_user_instructions() {
        let config = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };
        let provider = Arc::new(CaptureProvider {
            captured: std::sync::Mutex::new(None),
        });
        let result = run_decision_task(
            config,
            vec![ene_mind::HistoryEntry {
                role: ene_ai::Role::User,
                content: "hi".into(),
            }],
            ProactiveObservation::default(),
            None,
            ProactiveSuppressionState {
                seconds_since_user_input: 300,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
            QuietHoursEval::inactive(),
            None,
            Some(provider.clone() as Arc<dyn LlmProvider>),
            0,
            None,
            Vec::new(),
            vec!["don't talk while I work".to_string()],
            "en".to_string(),
        )
        .await;
        assert!(!result.should_generate);
        let captured = provider.captured.lock().expect("lock").clone();
        let text = captured.expect("decision provider must have been invoked");
        assert!(
            text.contains("\"user_instructions\":[\"don't talk while I work\"]"),
            "user instructions must reach the decision context JSON"
        );
    }

    struct FixedBodyProvider {
        body: String,
    }

    #[async_trait::async_trait]
    impl LlmProvider for FixedBodyProvider {
        fn name(&self) -> &'static str {
            "fixed-body"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                        > + Send,
                >,
            >,
            ene_ai::LlmProviderError,
        > {
            Err(ene_ai::LlmProviderError::Provider("stream unused".into()))
        }

        async fn chat_completion(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
            Ok(ene_ai::LlmCompletion::text_only(self.body.clone()))
        }
    }

    #[tokio::test]
    async fn decision_task_reports_confirmation_state() {
        let history = vec![ene_mind::HistoryEntry {
            role: ene_ai::Role::User,
            content: "hi".into(),
        }];
        let suppression = ProactiveSuppressionState {
            seconds_since_user_input: 300,
            seconds_since_proactive: 1000,
            proactive_turns_this_session: 0,
            user_turn_busy: false,
        };
        let speak = r#"{"should_speak":true,"confidence":0.9,"reason":"idle","topic_hint":"hi","urgency":"normal"}"#;
        let base = ProactiveConfig {
            enabled: true,
            min_idle_seconds: 0,
            cooldown_seconds: 0,
            max_turns_per_session: 5,
            ..ProactiveConfig::default()
        };

        let result = run_decision_task(
            base.clone(),
            history.clone(),
            ProactiveObservation::default(),
            None,
            suppression,
            QuietHoursEval::inactive(),
            None,
            Some(Arc::new(FixedBodyProvider {
                body: speak.to_string(),
            }) as Arc<dyn LlmProvider>),
            0,
            None,
            Vec::new(),
            Vec::new(),
            "en".to_string(),
        )
        .await;
        assert!(result.should_generate);
        assert_eq!(
            result.confirmation,
            ene_mind::ProactiveConfirmation::Disabled
        );

        let mut confirmed = base;
        confirmed.confirmation_enabled = true;
        let result = run_decision_task(
            confirmed,
            history,
            ProactiveObservation::default(),
            None,
            suppression,
            QuietHoursEval::inactive(),
            None,
            Some(Arc::new(FixedBodyProvider {
                body: speak.to_string(),
            }) as Arc<dyn LlmProvider>),
            0,
            None,
            Vec::new(),
            Vec::new(),
            "en".to_string(),
        )
        .await;
        assert!(result.should_generate);
        assert_eq!(
            result.confirmation,
            ene_mind::ProactiveConfirmation::Pending
        );
    }

    fn pending_decision() -> ProactiveDecisionResult {
        ProactiveDecisionResult {
            epoch: 0,
            should_generate: true,
            should_speak: true,
            confidence: 0.8,
            llm_invoked: true,
            topic_hint: String::new(),
            detail: String::new(),
            confirmation: ene_mind::ProactiveConfirmation::Pending,
            catch_up: false,
            pending_confirmation: None,
        }
    }

    #[test]
    fn resolve_confirmation_distinguishes_decline_accept_and_empty() {
        let pending = ene_mind::ProactiveConfirmation::Pending;
        assert_eq!(
            resolve_confirmation(&TerminalReason::Declined, pending, true),
            ene_mind::ProactiveConfirmation::Declined
        );
        assert_eq!(
            resolve_confirmation(&TerminalReason::Done, pending, true),
            ene_mind::ProactiveConfirmation::Accepted
        );
        assert_eq!(
            resolve_confirmation(&TerminalReason::Done, pending, false),
            ene_mind::ProactiveConfirmation::Empty,
            "a Done turn without visible text is neither accepted nor declined"
        );
        assert_eq!(
            resolve_confirmation(
                &TerminalReason::Done,
                ene_mind::ProactiveConfirmation::Disabled,
                true
            ),
            ene_mind::ProactiveConfirmation::Disabled
        );
        assert_eq!(
            resolve_confirmation(&TerminalReason::Cancelled, pending, false),
            pending
        );
        assert_eq!(
            resolve_confirmation(
                &TerminalReason::Failed {
                    message: "x".into(),
                },
                pending,
                false
            ),
            pending
        );
    }

    #[test]
    fn decline_applies_cooldown_without_consuming_budget() {
        let mut scheduler = ProactiveScheduler {
            proactive_turns: 2,
            ..ProactiveScheduler::default()
        };
        let decision = pending_decision();

        let verdict =
            apply_proactive_completion(&mut scheduler, &decision, &TerminalReason::Declined, false);
        assert_eq!(verdict, ene_mind::ProactiveConfirmation::Declined);
        assert!(
            scheduler.last_proactive_at.is_some(),
            "a decline must apply the cooldown"
        );
        assert_eq!(
            scheduler.proactive_turns, 2,
            "a decline must not consume the utterance budget"
        );
    }

    #[test]
    fn done_consumes_budget_and_resolves_acceptance() {
        let mut scheduler = ProactiveScheduler {
            proactive_turns: 1,
            ..ProactiveScheduler::default()
        };
        let decision = pending_decision();

        let verdict =
            apply_proactive_completion(&mut scheduler, &decision, &TerminalReason::Done, true);
        assert_eq!(verdict, ene_mind::ProactiveConfirmation::Accepted);
        assert_eq!(scheduler.proactive_turns, 2);
        assert!(scheduler.last_proactive_at.is_some());
    }

    #[test]
    fn confirmation_logging_emits_distinct_states() {
        use std::io::Write;

        struct Buffer(Arc<std::sync::Mutex<Vec<u8>>>);

        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("lock").extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        fn capture(emit: impl FnOnce()) -> String {
            let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
            let writer_buffer = Arc::clone(&buffer);
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || Buffer(Arc::clone(&writer_buffer)))
                .with_ansi(false)
                .finish();
            tracing::subscriber::with_default(subscriber, emit);
            String::from_utf8(buffer.lock().expect("lock").clone()).expect("utf8 log")
        }

        let decision = pending_decision();
        let declined = capture(|| {
            log_confirmation(&decision, ene_mind::ProactiveConfirmation::Declined);
        });
        assert!(declined.contains("declined"), "log: {declined}");
        let accepted = capture(|| {
            log_confirmation(&decision, ene_mind::ProactiveConfirmation::Accepted);
        });
        assert!(accepted.contains("agreement"), "log: {accepted}");
        let empty = capture(|| {
            log_confirmation(&decision, ene_mind::ProactiveConfirmation::Empty);
        });
        assert!(empty.contains("no speech"), "log: {empty}");
    }

    #[test]
    fn quiet_hours_queue_is_bounded_and_reset_with_session() {
        let mut scheduler = ProactiveScheduler::default();
        for i in 0..40 {
            scheduler.quiet_hours_queue.push_back(QueuedQuietHour {
                local_date: format!("2026-08-{:02}", i % 30 + 1),
                local_time: "22:00".into(),
            });
            if scheduler.quiet_hours_queue.len() > QUIET_HOURS_QUEUE_CAP {
                scheduler.quiet_hours_queue.pop_front();
            }
        }
        assert_eq!(scheduler.quiet_hours_queue.len(), QUIET_HOURS_QUEUE_CAP);
        assert_eq!(
            scheduler
                .quiet_hours_queue
                .front()
                .map(|e| e.local_date.as_str()),
            Some("2026-08-09"),
            "oldest entries must be dropped first"
        );

        scheduler.reset_session();
        assert!(scheduler.quiet_hours_queue.is_empty());
    }

    #[test]
    fn user_turn_started_discards_the_quiet_hours_queue() {
        let mut scheduler = ProactiveScheduler::default();
        scheduler.quiet_hours_queue.push_back(QueuedQuietHour {
            local_date: "2026-08-03".into(),
            local_time: "22:00".into(),
        });
        scheduler.on_user_turn_started();
        assert!(scheduler.quiet_hours_queue.is_empty());
    }

    #[test]
    fn quiet_hours_items_renders_compact_list() {
        let entries = vec![
            QueuedQuietHour {
                local_date: "2026-08-03".into(),
                local_time: "22:30".into(),
            },
            QueuedQuietHour {
                local_date: "2026-08-04".into(),
                local_time: "22:45".into(),
            },
        ];
        assert_eq!(
            quiet_hours_items(&entries),
            "2026-08-03 22:30, 2026-08-04 22:45"
        );
    }

    #[test]
    fn catch_up_hint_renders_items_in_both_languages() {
        let en = quiet_hours_catch_up_hint("2026-08-03 22:30", "en");
        assert!(en.contains("2026-08-03 22:30"), "hint: {en}");
        assert!(!en.contains("{items}"), "placeholder must be replaced");
        let ja = quiet_hours_catch_up_hint("2026-08-03 22:30", "ja");
        assert!(ja.contains("2026-08-03 22:30"), "hint: {ja}");
        assert!(!ja.contains("{items}"), "placeholder must be replaced");
    }

    #[test]
    fn quiet_hours_suppression_helpers_require_an_active_window() {
        let suppress = ene_mind::QuietHoursSuppressConfig::default();
        let inactive = QuietHoursEval::inactive();
        assert!(!quiet_hours_suppresses_tts(&inactive, suppress));
        assert!(!quiet_hours_suppresses_notifications(&inactive, suppress));

        let active = QuietHoursEval {
            active: true,
            ..QuietHoursEval::inactive()
        };
        assert!(quiet_hours_suppresses_tts(&active, suppress));
        assert!(quiet_hours_suppresses_notifications(&active, suppress));

        let tts_only = ene_mind::QuietHoursSuppressConfig {
            tts: true,
            notifications: false,
            decisions: false,
        };
        assert!(quiet_hours_suppresses_tts(&active, tts_only));
        assert!(!quiet_hours_suppresses_notifications(&active, tts_only));
    }

    fn pending_candidate() -> PendingConfirmationPrompt {
        PendingConfirmationPrompt {
            id: 7,
            title: "cats".into(),
            content: "user dislikes cats".into(),
            age_days: 5.0,
        }
    }

    fn decision_with_candidate() -> ProactiveDecisionResult {
        ProactiveDecisionResult {
            pending_confirmation: Some(pending_candidate()),
            ..pending_decision()
        }
    }

    #[test]
    fn delivered_confirmation_sets_the_asked_marker() {
        let mut scheduler = ProactiveScheduler::default();
        let decision = decision_with_candidate();

        let verdict =
            apply_proactive_completion(&mut scheduler, &decision, &TerminalReason::Done, true);
        assert_eq!(verdict, ene_mind::ProactiveConfirmation::Accepted);
        assert_eq!(
            scheduler.asked_pending_candidate,
            Some(pending_candidate()),
            "a delivered question must block re-selection until classified"
        );
        assert!(
            scheduler.pending_confirmation_asked_at.contains_key(&7),
            "delivery must arm the per-candidate re-ask backoff"
        );
    }

    #[test]
    fn delivery_sets_the_marker_even_without_the_confirmation_feature() {
        let mut scheduler = ProactiveScheduler::default();
        let decision = ProactiveDecisionResult {
            confirmation: ene_mind::ProactiveConfirmation::Disabled,
            ..decision_with_candidate()
        };
        let verdict =
            apply_proactive_completion(&mut scheduler, &decision, &TerminalReason::Done, true);
        assert_eq!(verdict, ene_mind::ProactiveConfirmation::Disabled);
        assert_eq!(
            scheduler.asked_pending_candidate,
            Some(pending_candidate()),
            "the question was delivered, so its reply must be classified"
        );
    }

    #[test]
    fn undelivered_confirmation_never_sets_the_asked_marker() {
        for (terminal, spoke) in [
            (TerminalReason::Declined, false),
            (TerminalReason::Done, false),
            (
                TerminalReason::Failed {
                    message: "boom".into(),
                },
                false,
            ),
            (TerminalReason::Cancelled, false),
        ] {
            let mut scheduler = ProactiveScheduler::default();
            let verdict = apply_proactive_completion(
                &mut scheduler,
                &decision_with_candidate(),
                &terminal,
                spoke,
            );
            assert_ne!(verdict, ene_mind::ProactiveConfirmation::Accepted);
            assert!(
                scheduler.asked_pending_candidate.is_none(),
                "an undelivered question ({verdict:?}) must not leave a marker"
            );
        }
    }

    #[test]
    fn ordinary_proactive_turns_do_not_touch_the_asked_marker() {
        let mut scheduler = ProactiveScheduler {
            asked_pending_candidate: Some(pending_candidate()),
            ..ProactiveScheduler::default()
        };
        let verdict = apply_proactive_completion(
            &mut scheduler,
            &pending_decision(),
            &TerminalReason::Done,
            true,
        );
        assert_eq!(verdict, ene_mind::ProactiveConfirmation::Accepted);
        assert_eq!(
            scheduler.asked_pending_candidate,
            Some(pending_candidate()),
            "a non-confirmation proactive turn must not consume the marker"
        );
    }

    #[test]
    fn session_reset_clears_the_asked_marker_but_keeps_backoff() {
        let mut scheduler = ProactiveScheduler {
            asked_pending_candidate: Some(pending_candidate()),
            pending_confirmation_asked_at: HashMap::from([(7, chrono::Utc::now())]),
            ..ProactiveScheduler::default()
        };
        let epoch = scheduler.session_epoch;
        scheduler.reset_session();
        assert!(scheduler.asked_pending_candidate.is_none());
        assert!(scheduler.pending_confirmation_asked_at.contains_key(&7));
        assert_eq!(scheduler.session_epoch, epoch.wrapping_add(1));
    }

    #[test]
    fn pending_confirmation_hint_renders_candidate_in_both_languages() {
        let en = proactive_pending_confirmation_hint(&pending_candidate(), "en", false);
        assert!(en.contains("cats: user dislikes cats"), "hint: {en}");
        assert!(!en.contains("{candidate}"), "placeholder must be replaced");

        let ja = proactive_pending_confirmation_hint(&pending_candidate(), "ja", false);
        assert!(ja.contains("cats: user dislikes cats"), "hint: {ja}");

        let with_note = proactive_pending_confirmation_hint(&pending_candidate(), "en", true);
        assert!(with_note.contains("<|silent|>"), "hint: {with_note}");
    }

    #[test]
    fn pending_confirmation_hint_truncates_long_candidate_text() {
        let candidate = PendingConfirmationPrompt {
            id: 1,
            title: "t".repeat(300),
            content: "c".repeat(900),
            age_days: 5.0,
        };
        let hint = proactive_pending_confirmation_hint(&candidate, "en", false);
        assert!(hint.contains(&format!("{}: {}", "t".repeat(160), "c".repeat(400))));
        assert!(!hint.contains(&"t".repeat(161)), "title must be truncated");
        assert!(
            !hint.contains(&"c".repeat(401)),
            "content must be truncated"
        );
    }

    #[test]
    fn pending_hint_fills_an_empty_confirmation_note_from_the_fallback() {
        let mut prompts = ene_config::prompts::ProactivePrompts {
            decision_system: String::new(),
            generation_hint_idle: String::new(),
            generation_hint_with_topic: String::new(),
            confirmation_note: String::new(),
            catch_up_note: String::new(),
            pending_confirmation_note: "ask: {candidate}".into(),
            pending_resolution_system: String::new(),
            screen_summary_system: String::new(),
            screen_summary_user: String::new(),
            screen_summary_layout_note: String::new(),
            screen_summary_code_note: String::new(),
            screen_summary_ocr_note: String::new(),
        };
        ensure_confirmation_note(&mut prompts, "en");
        assert!(
            prompts.confirmation_note.contains("<|silent|>"),
            "the refusal instruction must survive an empty pack note"
        );
        let hint = prompts.render_pending_confirmation_hint("cats", true);
        assert!(hint.contains("<|silent|>"));
        assert!(hint.contains("cats"));
    }

    #[tokio::test]
    async fn resolution_task_parses_verdicts_and_fails_closed() {
        let turn = TurnId::new();
        for (body, expected) in [
            (
                r#"{"verdict":"approved"}"#,
                PendingResolutionVerdict::Approved,
            ),
            (
                r#"{"verdict":"rejected"}"#,
                PendingResolutionVerdict::Rejected,
            ),
            (
                r#"{"verdict":"unclear"}"#,
                PendingResolutionVerdict::Unclear,
            ),
            ("not json", PendingResolutionVerdict::Unclear),
        ] {
            let provider = Arc::new(FixedBodyProvider {
                body: body.to_string(),
            }) as Arc<dyn LlmProvider>;
            let result = run_resolution_task(
                pending_candidate(),
                "yes".into(),
                turn.clone(),
                0,
                provider,
                "en".to_string(),
                Duration::from_secs(5),
            )
            .await;
            assert_eq!(result.verdict, expected, "body: {body}");
            assert_eq!(result.candidate_id, 7);
            assert_eq!(result.turn, turn);
            assert_eq!(result.session_epoch, 0);
        }
    }

    #[tokio::test]
    async fn resolution_task_returns_unclear_on_provider_error() {
        struct FailingProvider;

        #[async_trait::async_trait]
        impl LlmProvider for FailingProvider {
            fn name(&self) -> &'static str {
                "failing"
            }

            async fn create_chat_stream(
                &self,
                _messages: &[ene_ai::LlmMessage],
                _tools: &[ene_plugin_proto::ToolSpec],
            ) -> Result<
                std::pin::Pin<
                    Box<
                        dyn tokio_stream::Stream<
                                Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                            > + Send,
                    >,
                >,
                ene_ai::LlmProviderError,
            > {
                Err(ene_ai::LlmProviderError::Provider("stream unused".into()))
            }

            async fn chat_completion(
                &self,
                _messages: &[ene_ai::LlmMessage],
                _json_schema: Option<serde_json::Value>,
            ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
                Err(ene_ai::LlmProviderError::Provider("boom".into()))
            }
        }

        let provider = Arc::new(FailingProvider) as Arc<dyn LlmProvider>;
        let result = run_resolution_task(
            pending_candidate(),
            "yes".into(),
            TurnId::new(),
            0,
            provider,
            "en".to_string(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(result.verdict, PendingResolutionVerdict::Unclear);
    }
}
