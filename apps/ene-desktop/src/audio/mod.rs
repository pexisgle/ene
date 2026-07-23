//! Desktop audio integration: microphone capture, TTS playback, and
//! viseme lip-sync.
//!
//! The heavy native audio toolchain (`cpal` for capture, `rodio` for
//! playback) is gated behind the `voice` cargo feature. With the
//! feature disabled this module still compiles to inert stubs so the
//! text-only shell builds without `ALSA` / `PipeWire`.
//!
//! ## Data flow
//!
//! - **Capture** ([`capture`]): `cpal` input stream → `VadEngine` →
//!   `SttProvider::transcribe` → [`AiBridge::run`](crate::ai_bridge::AiBridge::run).
//! - **Playback** ([`playback`]): [`EneEvent::AudioChunk`](ene_runtime::EneEvent::AudioChunk)
//!   → `rodio` sink + [`VisemeDriver`](viseme_driver::VisemeDriver).
//! - **Viseme** ([`viseme_driver`]): smoothed mouth-shape weights read
//!   once per render frame and applied to the VRM expression layer.
//!
//! [`AudioState`] is a bevy resource holding the cross-thread state the
//! subsystems share (mic active, TTS playing for self-voice suppression,
//! selected mic device, and the AI config used to build providers).

#[cfg(feature = "voice")]
pub mod viseme_driver;

#[cfg(feature = "voice")]
pub mod capture;
#[cfg(feature = "voice")]
pub mod playback;

use std::sync::Arc;

/// A decoded TTS audio chunk forwarded from the AI bridge pump to the
/// playback subsystem.
///
/// Mirrors the payload of [`ene_runtime::EneEvent::AudioChunk`] minus
/// the turn / origin metadata the playback path does not need.
#[cfg_attr(
    not(feature = "voice"),
    expect(
        dead_code,
        reason = "payload fields are only read by the voice playback path"
    )
)]
#[derive(Debug, Clone)]
pub struct AudioChunkPayload {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz (e.g. 24000).
    pub sample_rate: u32,
    /// Whether this is the final audio chunk for the turn.
    pub is_final: bool,
}

/// Sender half used by the AI bridge pump to forward `AudioChunk`
/// events into the playback subsystem.
pub type AudioChunkSender = crossbeam_channel::Sender<AudioChunkPayload>;

/// Receiver half owned by the playback task.
#[cfg(feature = "voice")]
pub type AudioChunkReceiver = crossbeam_channel::Receiver<AudioChunkPayload>;

// ---------------------------------------------------------------------------
// Voice-only shared state (gated behind the `voice` feature).
// ---------------------------------------------------------------------------

#[cfg(feature = "voice")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "voice")]
use bevy_ecs::prelude::*;

#[cfg(feature = "voice")]
use parking_lot::Mutex;

#[cfg(feature = "voice")]
use viseme_driver::VisemeDriver;

/// Shared audio state, registered as a bevy resource.
///
/// The atomic flags are read/written from the audio callback threads,
/// the playback task, and the UI thread, so they use `Arc<AtomicBool>`
/// rather than plain fields. The `config` snapshot lets the mic toggle
/// build STT / VAD providers without reaching back into the settings UI.
#[cfg(feature = "voice")]
#[derive(Resource, Clone)]
pub struct AudioState {
    /// Whether microphone capture is currently active.
    pub mic_active: Arc<AtomicBool>,
    /// Whether TTS audio is currently playing. Used for self-voice
    /// suppression: capture mutes mic input while this is `true` so the
    /// character's own voice is not transcribed back into a turn.
    pub tts_playing: Arc<AtomicBool>,
    /// Selected microphone device name, or `None` for the OS default.
    pub mic_device: Option<String>,
    /// AI config snapshot used to resolve STT / VAD provider settings.
    pub config: ene_config::EneConfig,
}

#[cfg(feature = "voice")]
impl Default for AudioState {
    fn default() -> Self {
        Self {
            mic_active: Arc::new(AtomicBool::new(false)),
            tts_playing: Arc::new(AtomicBool::new(false)),
            mic_device: None,
            config: ene_config::EneConfig::default(),
        }
    }
}

#[cfg(feature = "voice")]
impl AudioState {
    /// Whether microphone capture is currently active.
    #[must_use]
    pub fn is_mic_active(&self) -> bool {
        self.mic_active.load(Ordering::Relaxed)
    }

    /// Whether TTS audio is currently playing.
    #[must_use]
    pub fn is_tts_playing(&self) -> bool {
        self.tts_playing.load(Ordering::Relaxed)
    }
}

/// Shared handle to the viseme driver.
///
/// The playback task calls [`VisemeDriver::feed_pcm`] as TTS chunks
/// arrive; the render loop locks the driver once per frame, calls
/// [`VisemeDriver::analyze_weights`], and applies the result to the
/// character's expression layer. `Arc`-backed so the playback thread
/// and the bevy world share one driver.
#[cfg(feature = "voice")]
#[derive(Resource, Clone, Default)]
pub struct VisemeState(pub Arc<Mutex<VisemeDriver>>);

#[cfg(feature = "voice")]
impl VisemeState {
    /// Feed a chunk of TTS PCM into the viseme analyzer.
    pub fn feed_pcm(&self, pcm: &[f32], sample_rate: u32) {
        self.0.lock().feed_pcm(pcm, sample_rate);
    }

    /// Analyze the buffered audio and return the smoothed mouth-shape
    /// weights, if any PCM has been fed.
    pub fn analyze_weights(&self) -> Option<ene_vrm::viseme::VisemeWeights> {
        self.0.lock().analyze_weights()
    }

    /// Clear the buffered audio and reset the smoothed weights.
    pub fn reset(&self) {
        self.0.lock().reset();
    }
}

/// Active microphone capture handle.
///
/// `None` when capture is stopped. Dropping the contained handle (or
/// calling [`capture::MicHandle::stop`]) closes the `cpal` stream and
/// clears the `mic_active` flag.
///
/// This is **not** a bevy resource: `cpal::Stream` is `!Send + !Sync`,
/// so it cannot live in the ECS world. The chat UI owns it directly
/// (see [`ChatUi`](crate::chat_ui::render::ChatUi)).
#[cfg(feature = "voice")]
pub type MicCaptureHandle = Option<capture::MicHandle>;

/// Toggle microphone capture on or off.
///
/// Reads the shared [`AudioState`] (config snapshot, selected device,
/// flags), the [`TokioHandle`](crate::resource::tokio::TokioHandle),
/// and the [`EventChannels`](crate::resource::event_channels::EventChannels)
/// sender from the world, then starts or stops capture accordingly.
/// The active handle is stored in `mic_handle` (owned by the chat UI,
/// since `cpal::Stream` is `!Send` and cannot be a bevy resource).
///
/// # Errors
///
/// Returns a human-readable error when the AI config is missing, STT is
/// disabled (`ai.stt.provider = "none"`), a provider fails to build, or
/// `cpal` cannot open the input device.
///
/// Without the `voice` feature this always errors so the UI can surface
/// that the text-only build has no microphone support.
#[cfg(feature = "voice")]
pub fn toggle_mic_capture(
    world: &mut bevy_ecs::world::World,
    ai: &Arc<crate::ai_bridge::AiBridge>,
    mic_handle: &mut MicCaptureHandle,
) -> Result<(), String> {
    // Clone the shared state up front so the immutable borrows are
    // released before we mutate the capture handle.
    let audio_state = world.resource::<AudioState>().clone();
    let tokio = world
        .resource::<crate::resource::tokio::TokioHandle>()
        .0
        .clone();
    let event_tx = world
        .resource::<crate::resource::event_channels::EventChannels>()
        .tx
        .clone();

    let already_active = audio_state.is_mic_active();

    if already_active {
        if let Some(mut handle) = mic_handle.take() {
            handle.stop();
        }
        return Ok(());
    }

    let ai_cfg = audio_state
        .config
        .get_section::<ene_ai::AiConfig>()
        .map_err(|e| e.to_string())?;

    let stt_resolved = ai_cfg
        .resolve_stt()
        .ok_or_else(|| "STT is disabled (set ai.stt.provider)".to_string())?;
    let vad_provider = if ai_cfg.vad.provider == "none" {
        ene_ai::silero_vad::PROVIDER_NAME.to_string()
    } else {
        ai_cfg.vad.provider.clone()
    };

    let stt = ene_ai::AudioProviderRegistry::create_stt_provider(
        &stt_resolved.provider,
        &audio_state.config,
    )
    .map_err(|e| e.to_string())?;
    let vad = ene_ai::AudioProviderRegistry::create_vad_engine(&vad_provider, &audio_state.config)
        .map_err(|e| e.to_string())?;

    let handle = capture::start_mic_capture(
        &audio_state,
        Arc::from(stt),
        vad,
        Arc::clone(ai),
        tokio,
        event_tx,
    )
    .map_err(|e| e.to_string())?;

    *mic_handle = Some(handle);
    Ok(())
}

/// Text-only stub: microphone capture is unavailable without the
/// `voice` feature.
#[cfg(not(feature = "voice"))]
pub fn toggle_mic_capture(
    _world: &mut bevy_ecs::world::World,
    _ai: &Arc<crate::ai_bridge::AiBridge>,
    _mic_handle: &mut Option<()>,
) -> Result<(), String> {
    Err("Microphone capture requires the `voice` feature".to_string())
}
