use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ene_body::{DuplexState, VoiceRuntime};
use ene_companion::SoulId;
use ene_kernel::{DisplayDepth, SpeechPresenter, TaskBinding};
use ene_plugin_ipc::{ProviderAuth, TtsRequest};
use ene_session::BodyId;
use serde_json::json;

use super::ws::CoreBus;
use crate::CoreDaemon;

const AUDIO_CHUNK: usize = 4_096;

/// Speaks assistant text through `ai.tasks.tts` and emits `audio.chunk` on the core bus.
pub struct PluginSpeech {
    core: Weak<CoreDaemon>,
    events: CoreBus,
}

impl PluginSpeech {
    #[must_use]
    pub fn new(core: &Arc<CoreDaemon>, events: CoreBus) -> Self {
        Self {
            core: Arc::downgrade(core),
            events,
        }
    }

    fn tts_binding(core: &CoreDaemon) -> TaskBinding {
        core.ai().lock().tasks.tts.clone()
    }
}

#[async_trait]
impl SpeechPresenter for PluginSpeech {
    async fn present_speech(&self, text: &str) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        let binding = Self::tts_binding(&core);
        if binding.is_unconfigured() {
            return;
        }
        let request = TtsRequest {
            text: text.to_owned(),
            voice: binding.voice.clone(),
            model: binding.model.clone(),
            base_url: binding.base_url.clone(),
            auth: ProviderAuth {
                api_key: core.secret_for("tts"),
            },
        };
        match core
            .supervisor()
            .synthesize_tts(&crate::plugin_profile::task_row_id("tts"), request)
            .await
        {
            Ok(audio) => {
                let soul = speaking_occupant(&core).map(|(soul, _)| soul);
                feed_playback(&core, &self.events, &audio.pcm, audio.sample_rate);
                let expression = soul
                    .as_ref()
                    .and_then(|soul| current_mood(&core.companion(), soul));
                emit_pcm(
                    &self.events,
                    &audio.pcm,
                    audio.sample_rate,
                    soul.as_ref(),
                    expression.as_deref(),
                );
            }
            Err(err) => tracing::warn!(error = %err, "tts provider failed"),
        }
    }
}

pub(crate) fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

pub(crate) fn emit_voice_state(events: &CoreBus, runtime: &VoiceRuntime) {
    events.emit(
        DisplayDepth::Surface,
        json!({
            "type": "voice.state",
            "state": runtime.state().as_str(),
            "barge_in": matches!(runtime.state(), DuplexState::Interrupting),
        }),
    );
}

pub(crate) fn emit_audio_abort(events: &CoreBus) {
    events.emit(
        DisplayDepth::Surface,
        json!({
            "type": "audio.chunk",
            "pcm": [],
            "sample_rate": 16_000,
            "is_final": true,
            "abort": true,
        }),
    );
}

fn feed_playback(core: &CoreDaemon, events: &CoreBus, pcm: &[f32], sample_rate: u32) {
    if pcm.is_empty() {
        return;
    }
    let Some((soul, body)) = speaking_occupant(core) else {
        return;
    };
    let mut first = true;
    for chunk in pcm.chunks(AUDIO_CHUNK) {
        let now = wall_clock_ms();
        let lips = core.with_voice(|voice| {
            let result = if first {
                first = false;
                voice.begin_playback(body, chunk, now, sample_rate)
            } else {
                Ok(voice.push_output_pcm(chunk, now, sample_rate))
            };
            match result {
                Ok(cmd) => {
                    emit_voice_state(events, voice);
                    Some(cmd)
                }
                Err(err) => {
                    tracing::debug!(error = %err, "tts duplex skipped");
                    None
                }
            }
        });
        if let Some(cmd) = lips {
            drop(core.stage().bus().push(soul, cmd));
            super::performance::flush_soul(core, events, soul);
        }
    }
}

fn speaking_occupant(core: &CoreDaemon) -> Option<(ene_session::SoulId, BodyId)> {
    let occupants = core.occupants();
    occupants
        .iter()
        .find_map(|(soul, body)| body.map(|id| (*soul, id)))
        .or_else(|| occupants.first().map(|(soul, _)| (*soul, BodyId::new())))
}

fn current_mood(companion: &ene_companion::CompanionRuntime, soul: &SoulId) -> Option<String> {
    let row = companion.soul(*soul).ok()?;
    let label = row.affect.mood_label;
    if label.is_empty() { None } else { Some(label) }
}

fn emit_pcm(
    events: &CoreBus,
    pcm: &[f32],
    sample_rate: u32,
    soul: Option<&SoulId>,
    expression: Option<&str>,
) {
    if pcm.is_empty() {
        return;
    }
    let soul_id = soul.map(std::string::ToString::to_string);
    let mut expression = expression.map(str::to_owned);
    let mut remaining = pcm.chunks(AUDIO_CHUNK).peekable();
    while let Some(chunk) = remaining.next() {
        let is_final = remaining.peek().is_none();
        let mut payload = json!({
            "type": "audio.chunk",
            "pcm": chunk,
            "sample_rate": sample_rate,
            "is_final": is_final,
            "abort": false,
        });
        if let Some(soul_id) = &soul_id {
            payload["soul_id"] = json!(soul_id);
        }
        if let Some(label) = expression.as_deref() {
            payload["expression"] = json!(label);
        }
        events.emit(DisplayDepth::Surface, payload);
        expression = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_chunk_carries_expression_and_later_chunks_do_not() {
        let bus = CoreBus::new(16);
        let mut rx = bus.subscribe();
        let pcm = vec![0.0_f32; AUDIO_CHUNK * 2];
        emit_pcm(&bus, &pcm, 24_000, None, Some("happy"));
        let events = crate::http::ws::tests_drain(&mut rx);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["expression"], "happy");
        assert!(events[0].get("soul_id").is_none());
        assert!(events[1].get("expression").is_none());
        assert_eq!(events[1]["is_final"], true);
    }

    #[test]
    fn missing_mood_omits_field_and_single_chunk_stays_final_with_cue() {
        let bus = CoreBus::new(16);
        let mut rx = bus.subscribe();
        emit_pcm(&bus, &[0.5], 16_000, None, None);
        let events = crate::http::ws::tests_drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(events[0].get("expression").is_none());
        assert_eq!(events[0]["is_final"], true);
    }
}
