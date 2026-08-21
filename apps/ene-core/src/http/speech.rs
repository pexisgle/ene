use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ene_body::{DuplexState, VoiceRuntime};
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
                feed_playback(&core, &self.events, &audio.pcm, audio.sample_rate);
                emit_pcm(&self.events, &audio.pcm, audio.sample_rate);
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

fn emit_pcm(events: &CoreBus, pcm: &[f32], sample_rate: u32) {
    if pcm.is_empty() {
        return;
    }
    let mut chunks: Vec<&[f32]> = pcm.chunks(AUDIO_CHUNK).collect();
    let last = chunks.pop();
    for chunk in chunks {
        events.emit(
            DisplayDepth::Surface,
            json!({
                "type": "audio.chunk",
                "pcm": chunk,
                "sample_rate": sample_rate,
                "is_final": false,
                "abort": false,
            }),
        );
    }
    if let Some(chunk) = last {
        events.emit(
            DisplayDepth::Surface,
            json!({
                "type": "audio.chunk",
                "pcm": chunk,
                "sample_rate": sample_rate,
                "is_final": true,
                "abort": false,
            }),
        );
    }
}
