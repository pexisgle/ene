//! Local ONNX provider plugin: Silero VAD sessions and capability
//! declarations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ene_ai::traits::VadEngine;
use ene_ai::{AudioProviderError, VadEvent as HostVadEvent};
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::config::VadConfig;

/// Builds a VAD engine for a fully-resolved config. Injectable so VAD
/// sessions can be tested without a model file or ONNX Runtime.
pub(crate) type EngineBuilder =
    Arc<dyn Fn(&VadConfig) -> Result<Box<dyn VadEngine>, PluginError> + Send + Sync>;

/// Maps a host-side VAD event onto the wire representation.
fn map_event_wire(event: HostVadEvent) -> VadEvent {
    match event {
        HostVadEvent::SpeechStart => VadEvent::SpeechStart,
        HostVadEvent::SpeechContinue => VadEvent::SpeechContinue,
        HostVadEvent::SpeechEnd => VadEvent::SpeechEnd,
        HostVadEvent::Silence => VadEvent::Silence,
    }
}

fn map_engine_error(e: &AudioProviderError) -> PluginError {
    PluginError::provider(format!("silero VAD failed: {e}"))
}

/// VAD plugin serving the Silero VAD ONNX engine.
///
/// The static capability data (`vad_spec()` / `VAD_PROVIDER_KIND`) comes
/// from the `#[provider(...)]` attribute. Engine state is per session: the
/// host generates a `session_id` per engine instance, and the plugin keeps
/// one `VadEngine` per id so recurrent state survives chunk boundaries.
/// `reset` discards the session, mirroring `VadEngine::reset`.
///
/// Capability declarations (`provides` / `requires`) are hand-written
/// because the derive only emits them for `LlmPlugin`; the strings are
/// static and validated by the host registry at handshake time.
#[derive(VadPlugin)]
#[provider(
    kind = "silero",
    // Silero VAD v5 operates on 512-sample (32 ms) chunks at 16 kHz.
    frame_size = 512,
    // One ONNX session runs one step at a time; the per-session capture loop
    // is inherently serial anyway.
    concurrency = 1,
    queue_depth = 2,
)]
pub struct OnnxPlugin {
    /// Per-session engines; `reset` removes the entry.
    engines: Arc<Mutex<HashMap<String, Box<dyn VadEngine>>>>,
    build: EngineBuilder,
}

impl OnnxPlugin {
    /// Creates the plugin with the real ONNX-backed engine builder.
    #[must_use]
    pub fn new() -> Self {
        Self::with_builder(Arc::new(build_real))
    }

    pub(crate) fn with_builder(build: EngineBuilder) -> Self {
        Self {
            engines: Arc::new(Mutex::new(HashMap::new())),
            build,
        }
    }
}

impl Default for OnnxPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ene_plugin::ConfigurablePlugin for OnnxPlugin {
    /// Advertises the settings surface for `plugins.list.onnx.config`.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "Silero VAD model name; used as a path fallback when model_path is unset"
                },
                "model_path": {
                    "type": "string",
                    "description": "Silero VAD ONNX model file path (defaults to the shared models cache)"
                },
                "threshold": {
                    "type": "number",
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "default": 0.5,
                    "description": "Speech probability threshold (0.0-1.0)"
                },
                "ort_dylib_path": {
                    "type": "string",
                    "description": "ONNX Runtime dynamic library path override (ort default resolution when unset). Fixed at process start: ONNX Runtime initializes once, so a change requires a restart"
                }
            }
        }))
    }
}

#[async_trait]
impl VadPlugin for OnnxPlugin {
    fn vad_capabilities(&self) -> Vec<VadProviderSpec> {
        vec![Self::vad_spec()]
    }

    async fn process_chunk(
        &self,
        kind: &str,
        config: Value,
        session_id: String,
        pcm: Vec<f32>,
        reset: bool,
    ) -> Result<VadEvent, PluginError> {
        if kind != Self::VAD_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("engine kind: {kind}")));
        }
        let frame_size = Self::vad_spec().frame_size as usize;
        if !reset && pcm.len() != frame_size {
            return Err(PluginError::provider(format!(
                "silero VAD expects {} samples per chunk, got {}",
                frame_size,
                pcm.len()
            )));
        }
        let config = VadConfig::from_value(&config)?;
        let engines = Arc::clone(&self.engines);
        let build = Arc::clone(&self.build);
        // The ONNX step is a short blocking call; run it off the server's
        // async worker so a wedged session cannot stall other requests.
        tokio::task::spawn_blocking(move || -> Result<VadEvent, PluginError> {
            let mut map = engines.lock().unwrap_or_else(PoisonError::into_inner);
            if reset {
                map.remove(&session_id);
                return Ok(VadEvent::Silence);
            }
            if !map.contains_key(&session_id) {
                let engine = (build)(&config)?;
                tracing::info!(
                    component = "ene-plugin-onnx",
                    session = %session_id,
                    "loaded Silero VAD engine for session"
                );
                map.insert(session_id.clone(), engine);
            }
            let engine = map
                .get_mut(&session_id)
                .ok_or_else(|| PluginError::provider("VAD session disappeared".to_string()))?;
            engine
                .process_chunk(&pcm)
                .map(map_event_wire)
                .map_err(|e| map_engine_error(&e))
        })
        .await
        .map_err(|e| PluginError::provider(format!("VAD engine task failed: {e}")))?
    }
}

fn build_real(config: &VadConfig) -> Result<Box<dyn VadEngine>, PluginError> {
    ene_voice::silero_vad::SileroVadEngine::open(
        &config.resolve_model_path(),
        config.threshold,
        config.ort_dylib_path.as_deref(),
    )
    .map(|engine| Box::new(engine) as Box<dyn VadEngine>)
    .map_err(|e| PluginError::provider(format!("silero VAD model init failed: {e}")))
}

/// Capabilities this plugin provides to other plugins: the shared ONNX
/// Runtime loading and the built-in English grapheme-to-phoneme rules.
#[must_use]
pub fn provides() -> Vec<CapabilityRef> {
    ["onnx-runner@1", "g2p/en@1"]
        .into_iter()
        .filter_map(|c| CapabilityRef::parse(c).ok())
        .collect()
}

/// Capabilities this plugin requires from other plugins: a Japanese G2P
/// provider is optional — the built-in rules fall back to English
/// phonemization when none is present.
#[must_use]
pub fn requires() -> Vec<CapabilityRequirement> {
    ["g2p/ja@^1?"]
        .into_iter()
        .filter_map(|c| CapabilityRequirement::parse(c).ok())
        .collect()
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;

    /// A scripted engine with deterministic events.
    struct ScriptedVad {
        events: Vec<HostVadEvent>,
        resets: u32,
    }

    impl VadEngine for ScriptedVad {
        fn frame_size(&self) -> usize {
            512
        }

        fn process_chunk(&mut self, _pcm: &[f32]) -> Result<HostVadEvent, AudioProviderError> {
            Ok(self
                .events
                .first()
                .copied()
                .unwrap_or(HostVadEvent::Silence))
        }

        fn reset(&mut self) {
            self.resets += 1;
        }

        fn name(&self) -> &'static str {
            "scripted"
        }
    }

    fn scripted_builder(events: Vec<HostVadEvent>) -> EngineBuilder {
        Arc::new(move |_config| {
            Ok(Box::new(ScriptedVad {
                events: events.clone(),
                resets: 0,
            }) as Box<dyn VadEngine>)
        })
    }

    fn config_value() -> Value {
        json!({"threshold": 0.5})
    }

    #[tokio::test]
    async fn process_chunk_returns_wire_event_and_caches_session() {
        let plugin = OnnxPlugin::with_builder(scripted_builder(vec![HostVadEvent::SpeechStart]));
        let first = plugin
            .process_chunk(
                OnnxPlugin::VAD_PROVIDER_KIND,
                config_value(),
                "s1".into(),
                vec![0.0; 512],
                false,
            )
            .await
            .expect("process");
        assert_eq!(first, VadEvent::SpeechStart);
        // Same session reuses the cached engine (still SpeechStart), a new
        // session builds a fresh one.
        let second = plugin
            .process_chunk(
                OnnxPlugin::VAD_PROVIDER_KIND,
                config_value(),
                "s1".into(),
                vec![0.0; 512],
                false,
            )
            .await
            .expect("process");
        assert_eq!(second, VadEvent::SpeechStart);
        assert_eq!(plugin.engines.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reset_removes_session_state() {
        let plugin = OnnxPlugin::with_builder(scripted_builder(vec![HostVadEvent::SpeechContinue]));
        plugin
            .process_chunk(
                OnnxPlugin::VAD_PROVIDER_KIND,
                config_value(),
                "s1".into(),
                vec![0.0; 512],
                false,
            )
            .await
            .expect("process");
        plugin
            .process_chunk(
                OnnxPlugin::VAD_PROVIDER_KIND,
                config_value(),
                "s1".into(),
                Vec::new(),
                true,
            )
            .await
            .expect("reset");
        assert!(plugin.engines.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn wrong_kind_and_frame_size_are_rejected() {
        let plugin = OnnxPlugin::with_builder(scripted_builder(vec![]));
        let err = plugin
            .process_chunk("other", config_value(), "s1".into(), vec![0.0; 512], false)
            .await
            .expect_err("wrong kind");
        assert!(err.to_string().contains("not supported"));
        let err = plugin
            .process_chunk(
                OnnxPlugin::VAD_PROVIDER_KIND,
                config_value(),
                "s1".into(),
                vec![0.0; 16],
                false,
            )
            .await
            .expect_err("wrong frame size");
        assert!(err.to_string().contains("expects 512 samples"));
    }

    #[test]
    fn capability_declarations_are_valid() {
        assert_eq!(
            provides(),
            ["onnx-runner@1", "g2p/en@1"]
                .into_iter()
                .filter_map(|c| CapabilityRef::parse(c).ok())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            requires(),
            ["g2p/ja@^1?"]
                .into_iter()
                .filter_map(|c| CapabilityRequirement::parse(c).ok())
                .collect::<Vec<_>>()
        );
    }
}
