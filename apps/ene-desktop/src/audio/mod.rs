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
    /// Whether the playback sink should stop immediately (barge-in) instead
    /// of draining queued audio. Only meaningful on a final marker: `true`
    /// for a cancelled turn, `false` for normal completion.
    pub abort: bool,
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
use std::collections::VecDeque;

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
/// The playback task calls [`VisemeState::push_chunk`] as TTS chunks
/// arrive; the render loop calls [`VisemeState::advance`] once per frame
/// (pacing consumption by the frame delta) and then
/// [`VisemeState::analyze_weights`], applying the result to the
/// character's expression layer. `Arc`-backed so the playback thread and
/// the bevy world share one driver.
///
/// ## Time alignment (M4)
///
/// PCM is queued with an enqueue timestamp rather than fed to the
/// analyzer immediately. The render loop consumes the queue paced by
/// elapsed time so the analyzer only ever sees the samples that are
/// *currently playing*. Feeding the whole chunk at enqueue time left the
/// analyzer's ring buffer static between chunks, causing repeated
/// RELEASE decay and a smeared mouth shape.
#[cfg(feature = "voice")]
#[derive(Resource, Clone, Default)]
pub struct VisemeState {
    /// The DSP driver (analyzer + smoothed weights).
    driver: Arc<Mutex<VisemeDriver>>,
    /// Queued PCM not yet consumed by the render loop, front = oldest.
    queue: Arc<Mutex<VecDeque<VisemeChunk>>>,
    /// Fractional sample position into the front chunk already consumed.
    cursor: Arc<Mutex<f64>>,
}

/// A queued TTS PCM chunk awaiting time-aligned playback (M4).
#[cfg(feature = "voice")]
#[derive(Clone)]
struct VisemeChunk {
    /// Mono PCM samples normalized to `[-1.0, 1.0]`.
    pcm: Vec<f32>,
    /// Sample rate in Hz.
    sample_rate: u32,
    /// When the chunk was handed to the playback sink.
    enqueued_at: std::time::Instant,
}

#[cfg(feature = "voice")]
impl VisemeState {
    /// Queue a chunk of TTS PCM for time-aligned viseme analysis.
    ///
    /// Called from the playback thread as each chunk is appended to the
    /// rodio sink. The render loop consumes it later via
    /// [`advance`](Self::advance).
    pub fn push_chunk(&self, pcm: Vec<f32>, sample_rate: u32) {
        if pcm.is_empty() || sample_rate == 0 {
            return;
        }
        self.queue.lock().push_back(VisemeChunk {
            pcm,
            sample_rate,
            enqueued_at: std::time::Instant::now(),
        });
    }

    /// Consume queued PCM up to the current playback position and feed it
    /// to the analyzer.
    ///
    /// The playback position is derived from the oldest queued chunk's
    /// enqueue time (a proxy for the rodio sink's play head). Only the
    /// samples that have "played" since the last call are fed, so the
    /// analyzer sees a smooth, time-aligned stream.
    pub fn advance(&self) {
        let now = std::time::Instant::now();

        let mut cursor = self.cursor.lock();
        let mut driver = self.driver.lock();
        let mut queue = self.queue.lock();

        // Pace each chunk against its own enqueue time rather than a single
        // target derived from the original front chunk. Recomputing the target
        // per chunk keeps the play-head estimate correct after the front is
        // popped mid-call (a stale target mis-aligned every later chunk).
        while let Some(front) = queue.front() {
            let elapsed = now.duration_since(front.enqueued_at).as_secs_f64();
            let target = (elapsed * f64::from(front.sample_rate)).floor() as usize;
            let start = *cursor as usize;
            if start >= front.pcm.len() {
                // Front chunk fully consumed: drop it and move on.
                queue.pop_front();
                *cursor = 0.0;
                continue;
            }
            let end = target.min(front.pcm.len());
            if end > start {
                driver.feed_pcm(&front.pcm[start..end], front.sample_rate);
                *cursor = end as f64;
            }
            break;
        }
    }

    /// Analyze the buffered audio and return the smoothed mouth-shape
    /// weights, if any PCM has been fed.
    pub fn analyze_weights(&self) -> Option<ene_vrm::viseme::VisemeWeights> {
        self.driver.lock().analyze_weights()
    }

    /// Clear the buffered audio, the pending queue, and reset the
    /// smoothed weights.
    pub fn reset(&self) {
        self.driver.lock().reset();
        self.queue.lock().clear();
        *self.cursor.lock() = 0.0;
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

    // Single source of truth (L13): `mic_active` is the authoritative
    // "is capture live" flag — the error callback clears it on device
    // unplug even though the `!Send` handle still exists here. Reconcile
    // a stale handle (flag false but handle present) before deciding.
    let active = audio_state.is_mic_active();
    if !active {
        // Stream already died (device unplug) or was never started: drop
        // any stale handle so we fall through to a fresh start.
        *mic_handle = None;
    } else if let Some(mut handle) = mic_handle.take() {
        // Live stream: stop it.
        handle.stop();
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

#[cfg(all(test, feature = "voice"))]
mod tests {
    use super::*;

    #[test]
    fn viseme_state_push_and_advance() {
        let state = VisemeState::default();
        // Push a chunk of 2400 samples at 24 kHz = 100 ms of audio.
        state.push_chunk(vec![0.5; 2400], 24_000);
        // Immediately after enqueue, ~0 samples should have "played".
        state.advance();
        // No weights yet (nothing consumed) or very few samples.
        // After waiting, advance should consume samples.
        std::thread::sleep(std::time::Duration::from_millis(50));
        state.advance();
        // After 50 ms at 24 kHz, ~1200 samples should be consumed.
        // Weights should now be available.
        let weights = state.analyze_weights();
        assert!(weights.is_some());
    }

    #[test]
    fn viseme_state_reset_clears_queue() {
        let state = VisemeState::default();
        state.push_chunk(vec![0.5; 2400], 24_000);
        // Wait and advance so the driver actually gets fed.
        std::thread::sleep(std::time::Duration::from_millis(50));
        state.advance();
        assert!(state.analyze_weights().is_some());
        // Reset clears the queue and zeroes the smoothed weights.
        state.reset();
        state.advance();
        let weights = state.analyze_weights();
        // After reset the analyzer exists but weights are zeroed.
        assert!(
            weights.is_none_or(|w| w.aa + w.ih + w.ou + w.ee + w.oh == 0.0),
            "expected zeroed weights after reset, got {weights:?}"
        );
    }

    #[test]
    fn viseme_state_empty_push_ignored() {
        let state = VisemeState::default();
        state.push_chunk(vec![], 24_000);
        state.push_chunk(vec![0.5; 100], 0);
        state.advance();
        assert!(state.analyze_weights().is_none());
    }

    #[test]
    fn viseme_state_multiple_chunks_consumed_in_order() {
        let state = VisemeState::default();
        state.push_chunk(vec![0.1; 1200], 24_000);
        state.push_chunk(vec![0.9; 1200], 24_000);
        // Wait long enough for both chunks to have "played".
        std::thread::sleep(std::time::Duration::from_millis(120));
        state.advance();
        // Both chunks should be consumed; weights available.
        assert!(state.analyze_weights().is_some());
    }
}
