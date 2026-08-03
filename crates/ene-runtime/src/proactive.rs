use ene_ai::LlmProvider;
use ene_mind::{
    ActiveCommitmentPrompt, ProactiveConfig, ProactiveObservation, ProactiveSuppressionState,
    ScreenSummaryStatus, build_proactive_context, decide_proactive_speech,
};
use ene_store::AffectState as StoreAffectState;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    /// Tick counter for periodic world state memory writes.
    #[expect(dead_code, reason = "planned for #209 world-state persistence")]
    pub world_state_tick: usize,
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
            world_state_tick: 0,
        }
    }
}

impl ProactiveScheduler {
    /// Record that a user turn began (cancels stale decisions via epoch bump).
    pub fn on_user_turn_started(&mut self) {
        self.last_user_input_at = Instant::now();
        self.epoch = self.epoch.wrapping_add(1);
    }

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
        self.last_screen_image_data_uri = None;
        self.last_user_input_at = Instant::now();
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
    /// Tick counter for periodic world state memory writes.
    #[expect(dead_code, reason = "planned for #209 world-state persistence")]
    pub world_state_tick: usize,
    pub should_generate: bool,
    /// Model `should_speak` flag (before confidence gate).
    pub should_speak: bool,
    /// Model confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    pub llm_invoked: bool,
    pub topic_hint: String,
    pub detail: String,
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
    suppression: ProactiveSuppressionState,
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
    );
    let outcome = decide_proactive_speech(&config, &context, provider, &prompt_language).await;
    let should_generate = outcome.skip.is_none()
        && outcome
            .decision
            .allows_generation(config.decision.min_confidence);
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
        world_state_tick: 0,
        should_generate,
        should_speak: outcome.decision.should_speak,
        confidence: outcome.decision.confidence,
        llm_invoked: outcome.llm_invoked,
        topic_hint: outcome.decision.topic_hint,
        detail,
    }
}

/// Build the internal generation hint (never stored as a user message).
#[must_use]
pub(crate) fn proactive_generation_hint(topic_hint: &str, prompt_language: &str) -> String {
    ene_config::PromptLibrary::load(prompt_language)
        .proactive()
        .render_generation_hint(topic_hint)
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
            ProactiveSuppressionState {
                seconds_since_user_input: 300,
                seconds_since_proactive: 1000,
                proactive_turns_this_session: 0,
                user_turn_busy: false,
            },
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
}
