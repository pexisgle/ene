use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_kernel::{DisplayDepth, SpeechPresenter, TaskBinding};
use ene_plugin_ipc::{ProviderAuth, TtsRequest};
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
            .synthesize_tts(&binding.plugin, request)
            .await
        {
            Ok(audio) => emit_pcm(&self.events, &audio.pcm, audio.sample_rate),
            Err(err) => tracing::warn!(error = %err, "tts provider failed"),
        }
    }
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
