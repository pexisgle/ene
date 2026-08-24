//! TTS audio playback via `rodio`.
//!
//! A dedicated OS thread owns the cpal output stream, rodio mixer, and
//! player (all must stay alive on the thread that created them). The core
//! session pump forwards [`AudioChunkPayload`] values from WS `audio.chunk`
//! events over a [`crossbeam_channel`]; the playback thread appends
//! each chunk to the sink, feeds the same PCM to the shared
//! [`VisemeState`] for lip-sync, and toggles the
//! `tts_playing` flag used for self-voice suppression. Chunks that carry
//! sentence-scoped expression cues ([`AudioChunkPayload::cues`]) also fire
//! those cues on the emotion pipeline timed to when that sentence's audio
//! starts playing. A barge-in (`abort` final) additionally cancels the
//! pipeline's scheduled expressions, whose audio was just discarded.
//!
//! The playback thread exits when the channel closes **or** when the
//! shared shutdown flag is raised. The flag is necessary because the
//! channel sender is cloned into the AI bridge pump task, so dropping
//! the `AppState`'s own sender does not close the channel.
//!
//! Gated behind the `voice` feature; without it the desktop builds a
//! text-only shell and this module is not compiled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rodio::Player;
use rodio::buffer::SamplesBuffer;
use rodio::mixer::{self, Mixer};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{AudioChunkPayload, AudioChunkReceiver, AudioChunkSender, VisemeState};
use crate::events::{AppEvent, AppEventSender};

/// Wall-clock ↔ audio-timeline mapping used to schedule expression cues.
///
/// Models the rodio sink as a continuous player: the first chunk appended in
/// a playback segment starts playing immediately, and timeline position `p`
/// (cumulative appended PCM seconds) plays at wall time
/// `segment_started_at + p`. The anchor resets after every final marker
/// (barge-in stop or natural drain) because the sink then restarts from
/// silence. This is the same enqueue-time proxy the viseme driver uses for
/// lip-sync pacing.
#[derive(Debug)]
struct PlaybackTimeline {
    segment_started_at: Option<Instant>,
    appended_secs: f64,
}

impl PlaybackTimeline {
    const fn new() -> Self {
        Self {
            segment_started_at: None,
            appended_secs: 0.0,
        }
    }

    /// Starts a new playback segment at `now` (first chunk after a final
    /// marker: the sink restarts from silence).
    fn on_segment_start(&mut self, now: Instant) {
        self.segment_started_at = Some(now);
        self.appended_secs = 0.0;
    }

    /// Wall time at which the next appended chunk starts playing.
    fn next_chunk_start(&self) -> Option<Instant> {
        self.segment_started_at
            .map(|anchor| anchor + Duration::from_secs_f64(self.appended_secs))
    }

    fn append(&mut self, pcm_len: usize, sample_rate: u32) {
        self.appended_secs += pcm_len as f64 / f64::from(sample_rate);
    }

    fn reset(&mut self) {
        self.segment_started_at = None;
        self.appended_secs = 0.0;
    }
}

/// Schedules an expression cue on the emotion pipeline, timed to the audio.
///
/// `fire_wall` is the wall time at which the sentence's audio starts playing;
/// the cue is scheduled there (or immediately when the sink has already
/// passed it). `clock_origin` is the app-wide monotonic origin the emotion
/// pipeline ticks against (`AppState::clock_origin`), so the cue pops on the
/// frame at or after the sentence's audio start — at most one frame late,
/// never early.
fn fire_cues(
    cues: &[super::ExpressionCue],
    fire_wall: Option<Instant>,
    clock_origin: Instant,
    event_tx: &AppEventSender,
) {
    let delay = fire_wall.map_or(0.0, |wall| {
        wall.saturating_duration_since(Instant::now()).as_secs_f64()
    });
    let target_time = clock_origin.elapsed().as_secs_f64() + delay;
    for cue in cues {
        drop(event_tx.send(AppEvent::ExpressionCue {
            name: cue.name.clone(),
            weight: cue.weight,
            hold_secs: cue.hold_secs,
            target_time,
        }));
    }
}

/// Manually assembled audio sink: a rodio [`Mixer`] fed into a cpal output
/// stream, with a [`Player`] for queueing sounds.
struct AudioSink {
    _mixer: Mixer,
    player: Player,
    _stream: cpal::Stream,
}

fn open_audio_sink() -> Result<AudioSink, String> {
    use std::num::NonZero;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default audio output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("failed to get default output config: {e}"))?;

    let channels = NonZero::new(supported.channels()).ok_or("output device has zero channels")?;
    let sample_rate =
        NonZero::new(supported.sample_rate()).ok_or("output device has zero sample rate")?;

    let (mixer, mut mixer_source) = mixer::mixer(channels, sample_rate);

    let err_fn = move |e: cpal::Error| {
        tracing::error!(component = "AudioPlayback", error = %e, "audio output stream error");
    };

    let stream = device
        .build_output_stream::<f32, _, _>(
            supported.config(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for sample in data.iter_mut() {
                    *sample = mixer_source.next().unwrap_or(0.0);
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| format!("failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("failed to play output stream: {e}"))?;

    let player = Player::connect_new(&mixer);

    Ok(AudioSink {
        _mixer: mixer,
        player,
        _stream: stream,
    })
}

/// How long the playback loop waits on the channel before re-checking
/// the shutdown flag. Bounds the join latency on drop.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum number of PCM chunks buffered between the AI bridge pump and
/// the playback thread. Bounds memory growth when the sink cannot
/// keep up; the pump drops the oldest chunk once the buffer is full.
pub(crate) const PLAYBACK_CHANNEL_CAPACITY: usize = 64;

/// Handle to the running playback thread.
///
/// Dropping the handle raises the shutdown flag and joins the thread.
/// The thread exits once the paired [`AudioChunkSender`] is dropped
/// (channel closed) **or** the shutdown flag is raised — whichever
/// comes first. The flag is required because a second sender clone
/// lives in the AI bridge pump task, so the channel may stay open even
/// after `AppState` drops its own sender.
pub struct AudioPlaybackHandle {
    join: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl AudioPlaybackHandle {
    /// Stop playback and wait for the playback thread to exit.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            drop(join.join());
        }
    }
}

impl Drop for AudioPlaybackHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Create the playback channel and spawn the playback thread.
///
/// Returns the sender half (handed to the AI bridge pump so it can
/// forward `AudioChunk` events) and the handle that keeps the thread
/// alive. `clock_origin` is the app-wide monotonic origin
/// (`AppState::clock_origin`) that the emotion pipeline also ticks
/// against, so cues scheduled here pop exactly when the matching audio
/// starts.
pub fn spawn_playback(
    viseme: VisemeState,
    tts_playing: Arc<AtomicBool>,
    cue_events: AppEventSender,
    clock_origin: Instant,
) -> (AudioChunkSender, AudioPlaybackHandle) {
    let (tx, rx) = crossbeam_channel::bounded::<AudioChunkPayload>(PLAYBACK_CHANNEL_CAPACITY);
    let shutdown = Arc::new(AtomicBool::new(false));
    let loop_shutdown = Arc::clone(&shutdown);
    let join = std::thread::Builder::new()
        .name("ene-audio-playback".to_string())
        .spawn(move || {
            playback_loop(
                rx,
                viseme,
                tts_playing,
                loop_shutdown,
                cue_events,
                clock_origin,
            );
        })
        .ok();
    (tx, AudioPlaybackHandle { join, shutdown })
}

/// The playback thread body.
///
/// Opens the default output device once, then drains the channel until
/// it closes or the shutdown flag is raised. If no output device is
/// available the loop still drains the channel (discarding audio) so
/// the sender never blocks, but logs a warning and leaves `tts_playing`
/// untouched.
fn playback_loop(
    rx: AudioChunkReceiver,
    viseme: VisemeState,
    tts_playing: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    cue_events: AppEventSender,
    clock_origin: Instant,
) {
    let sink = match open_audio_sink() {
        Ok(sink) => Some(sink),
        Err(e) => {
            tracing::warn!(
                component = "AudioPlayback",
                error = %e,
                "failed to open audio output; TTS audio disabled"
            );
            None
        }
    };

    let Some(sink) = sink else {
        // No audio backend: keep draining so the bounded channel sender
        // never blocks.
        drain_until_shutdown(&rx, &viseme, &shutdown, &cue_events, clock_origin);
        return;
    };

    let mut state = PlaybackState::new();
    let mut timeline = PlaybackTimeline::new();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let chunk = match rx.recv_timeout(SHUTDOWN_POLL_INTERVAL) {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };

        match state.process_chunk(&chunk) {
            PlaybackAction::Final {
                was_speaking,
                abort,
            } => {
                // Final marker: release the self-voice suppression flag and
                // reset lip-sync. On a barge-in (`abort`) stop the sink
                // immediately so queued audio is discarded; otherwise let the
                // utterance drain naturally before resetting.
                if abort {
                    sink.player.stop();
                    cancel_scheduled_cues(&cue_events);
                } else if was_speaking {
                    sink.player.sleep_until_end();
                }
                tts_playing.store(false, Ordering::Relaxed);
                viseme.reset();
                timeline.reset();
            }
            PlaybackAction::Audio { first } => {
                if first {
                    // The sink restarted from silence (previous segment was
                    // finalized): the new segment's timeline begins now.
                    timeline.on_segment_start(Instant::now());
                }
                // The chunk's audio starts at the timeline position before
                // its own span; cues riding this chunk mark a sentence start
                // and must fire when that sentence's audio begins.
                let fire_wall = timeline.next_chunk_start();
                if !chunk.cues.is_empty() {
                    fire_cues(&chunk.cues, fire_wall, clock_origin, &cue_events);
                }
                timeline.append(chunk.pcm.len(), chunk.sample_rate);

                // Queue the PCM for time-aligned viseme analysis. The
                // render loop consumes it paced by the playback clock rather
                // than feeding the whole chunk at enqueue time.
                viseme.push_chunk(chunk.pcm.clone(), chunk.sample_rate);

                #[expect(
                    clippy::unwrap_used,
                    reason = "hardcoded mono (1) and validated sample_rate > 0"
                )]
                let buffer = SamplesBuffer::new(
                    std::num::NonZero::new(1u16).unwrap(),
                    std::num::NonZero::new(chunk.sample_rate).unwrap(),
                    chunk.pcm,
                );
                sink.player.append(buffer);

                if first {
                    tts_playing.store(true, Ordering::Relaxed);
                }
            }
            PlaybackAction::Skip => {}
        }
    }

    tts_playing.store(false, Ordering::Relaxed);
    viseme.reset();
}

/// Asks the emotion pipeline to drop its scheduled and active expression
/// state (`CancelCommand("expr")` semantics). Sent when a barge-in abort
/// discards the queued audio: cues already scheduled for that audio are
/// stale and would otherwise pop during the next turn.
fn cancel_scheduled_cues(cue_events: &AppEventSender) {
    drop(cue_events.send(AppEvent::CancelCue {
        scope: "expr".to_string(),
    }));
}

/// Drain-and-discard loop used when no audio output device is available.
///
/// Expression cues still fire (immediately — there is no audio timeline to
/// sync to), so a headless run keeps some expression behavior instead of
/// dropping markers silently.
fn drain_until_shutdown(
    rx: &AudioChunkReceiver,
    viseme: &VisemeState,
    shutdown: &Arc<AtomicBool>,
    cue_events: &AppEventSender,
    clock_origin: Instant,
) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(SHUTDOWN_POLL_INTERVAL) {
            Ok(chunk) => {
                if chunk.is_final {
                    viseme.reset();
                    if chunk.abort {
                        cancel_scheduled_cues(cue_events);
                    }
                }
                if !chunk.cues.is_empty() {
                    fire_cues(&chunk.cues, None, clock_origin, cue_events);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Playback state machine tracking whether audio is currently speaking.
///
/// Extracted from [`playback_loop`] so the `tts_playing` transitions can
/// be unit-tested without a real audio device.
struct PlaybackState {
    speaking: bool,
}

impl PlaybackState {
    fn new() -> Self {
        Self { speaking: false }
    }

    fn process_chunk(&mut self, chunk: &AudioChunkPayload) -> PlaybackAction {
        if chunk.is_final {
            let was_speaking = self.speaking;
            self.speaking = false;
            return PlaybackAction::Final {
                was_speaking,
                abort: chunk.abort,
            };
        }
        if chunk.pcm.is_empty() || chunk.sample_rate == 0 {
            return PlaybackAction::Skip;
        }
        let first = !self.speaking;
        self.speaking = true;
        PlaybackAction::Audio { first }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PlaybackAction {
    /// Chunk is a final marker; `was_speaking` indicates whether the
    /// sink needs to drain before resetting, and `abort` requests an
    /// immediate `sink.stop()` (barge-in) instead of draining.
    Final { was_speaking: bool, abort: bool },
    /// Chunk carries audio; `first` indicates this is the first chunk
    /// of the utterance (set `tts_playing`).
    Audio { first: bool },
    /// Chunk is empty or has an invalid sample rate; ignore it.
    Skip,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(pcm: Vec<f32>, sample_rate: u32, is_final: bool) -> AudioChunkPayload {
        AudioChunkPayload {
            pcm,
            sample_rate,
            is_final,
            abort: false,
            cues: Vec::new(),
        }
    }

    fn final_chunk(abort: bool) -> AudioChunkPayload {
        AudioChunkPayload {
            pcm: Vec::new(),
            sample_rate: 0,
            is_final: true,
            abort,
            cues: Vec::new(),
        }
    }

    #[test]
    fn timeline_no_segment_has_no_sentence_start() {
        let timeline = PlaybackTimeline::new();
        assert!(timeline.next_chunk_start().is_none());
    }

    #[test]
    fn timeline_sentence_start_follows_appended_duration() {
        let now = Instant::now();
        let mut timeline = PlaybackTimeline::new();
        timeline.on_segment_start(now);
        // 24,000 samples at 24 kHz = 1 second of audio.
        timeline.append(24_000, 24_000);
        assert_eq!(
            timeline.next_chunk_start(),
            Some(now + Duration::from_secs(1))
        );
    }

    #[test]
    fn timeline_first_chunk_starts_immediately() {
        let now = Instant::now();
        let mut timeline = PlaybackTimeline::new();
        timeline.on_segment_start(now);
        assert_eq!(timeline.next_chunk_start(), Some(now));
    }

    #[test]
    fn timeline_reset_clears_segment() {
        let now = Instant::now();
        let mut timeline = PlaybackTimeline::new();
        timeline.on_segment_start(now);
        timeline.append(24_000, 24_000);
        timeline.reset();
        assert!(timeline.next_chunk_start().is_none());
        timeline.on_segment_start(now + Duration::from_secs(5));
        assert_eq!(
            timeline.next_chunk_start(),
            Some(now + Duration::from_secs(5))
        );
    }

    #[test]
    fn timeline_empty_chunk_advances_nothing() {
        let now = Instant::now();
        let mut timeline = PlaybackTimeline::new();
        timeline.on_segment_start(now);
        timeline.append(0, 24_000);
        assert_eq!(timeline.next_chunk_start(), Some(now));
    }

    #[test]
    fn timeline_appends_accumulate() {
        let now = Instant::now();
        let mut timeline = PlaybackTimeline::new();
        timeline.on_segment_start(now);
        timeline.append(12_000, 24_000);
        timeline.append(12_000, 24_000);
        assert_eq!(
            timeline.next_chunk_start(),
            Some(now + Duration::from_secs(1))
        );
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
    fn fire_cues_schedules_expression_and_clamps_negative_delay() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        let epoch = Instant::now();
        let cues = vec![super::super::ExpressionCue::expression("happy")];
        // Past fire wall (sink ahead of the timeline): fires immediately.
        fire_cues(&cues, Some(epoch), epoch, &tx);
        let event = rx.try_recv().expect("expression cue must be sent");
        assert!(
            matches!(
                &event,
                AppEvent::ExpressionCue {
                    name,
                    weight,
                    hold_secs,
                    target_time
                } if name == "happy" && *weight == 1.0_f32 && *hold_secs == 4.0_f64
                    && *target_time >= 0.0 && *target_time < 1.0
            ),
            "expected an immediate happy cue, got {event:?}"
        );
        assert!(
            rx.try_recv().is_err(),
            "motion cues must not be forwarded by the expression path"
        );
    }

    #[test]
    fn fire_cues_schedules_future_cue_at_wall_time() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        let epoch = Instant::now();
        let future = Instant::now() + Duration::from_secs(5);
        fire_cues(
            &[super::super::ExpressionCue::expression("sad")],
            Some(future),
            epoch,
            &tx,
        );
        let event = rx.try_recv().expect("expression cue must be sent");
        assert!(
            matches!(
                &event,
                AppEvent::ExpressionCue { target_time, .. }
                    if (4.5..=5.5).contains(target_time)
            ),
            "target_time should sit ~5s on the epoch, got {event:?}"
        );
    }

    #[test]
    fn fire_cues_without_fire_wall_fires_immediately() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        let epoch = Instant::now();
        fire_cues(
            &[super::super::ExpressionCue::expression("happy")],
            None,
            epoch,
            &tx,
        );
        let event = rx.try_recv().expect("cue must be sent");
        assert!(
            matches!(
                &event,
                AppEvent::ExpressionCue { target_time, .. }
                    if *target_time >= 0.0 && *target_time < 1.0
            ),
            "expected an immediate cue, got {event:?}"
        );
    }

    #[test]
    fn cancel_scheduled_cues_sends_expr_cancel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        cancel_scheduled_cues(&tx);
        let event = rx.try_recv().expect("cancel cue must be sent");
        assert!(
            matches!(&event, AppEvent::CancelCue { scope } if scope == "expr"),
            "expected an expr cancel cue, got {event:?}"
        );
    }

    #[test]
    fn playback_state_first_chunk_sets_playing() {
        let mut state = PlaybackState::new();
        let action = state.process_chunk(&chunk(vec![0.1; 100], 24_000, false));
        assert_eq!(action, PlaybackAction::Audio { first: true });
        assert!(state.speaking);
    }

    #[test]
    fn playback_state_subsequent_chunk_not_first() {
        let mut state = PlaybackState::new();
        state.process_chunk(&chunk(vec![0.1; 100], 24_000, false));
        let action = state.process_chunk(&chunk(vec![0.2; 100], 24_000, false));
        assert_eq!(action, PlaybackAction::Audio { first: false });
    }

    #[test]
    fn playback_state_final_clears_speaking() {
        let mut state = PlaybackState::new();
        state.process_chunk(&chunk(vec![0.1; 100], 24_000, false));
        let action = state.process_chunk(&chunk(vec![], 0, true));
        assert_eq!(
            action,
            PlaybackAction::Final {
                was_speaking: true,
                abort: false
            }
        );
        assert!(!state.speaking);
    }

    #[test]
    fn playback_state_final_without_audio() {
        let mut state = PlaybackState::new();
        let action = state.process_chunk(&chunk(vec![], 0, true));
        assert_eq!(
            action,
            PlaybackAction::Final {
                was_speaking: false,
                abort: false
            }
        );
    }

    #[test]
    fn playback_state_final_abort_propagates() {
        let mut state = PlaybackState::new();
        state.process_chunk(&chunk(vec![0.1; 100], 24_000, false));
        let action = state.process_chunk(&final_chunk(true));
        assert_eq!(
            action,
            PlaybackAction::Final {
                was_speaking: true,
                abort: true
            }
        );
        assert!(!state.speaking);
    }

    #[test]
    fn playback_state_empty_chunk_skipped() {
        let mut state = PlaybackState::new();
        let action = state.process_chunk(&chunk(vec![], 24_000, false));
        assert_eq!(action, PlaybackAction::Skip);
        assert!(!state.speaking);
    }

    #[test]
    fn playback_state_zero_rate_skipped() {
        let mut state = PlaybackState::new();
        let action = state.process_chunk(&chunk(vec![0.1; 100], 0, false));
        assert_eq!(action, PlaybackAction::Skip);
        assert!(!state.speaking);
    }

    #[test]
    fn playback_state_full_lifecycle() {
        let mut state = PlaybackState::new();
        assert_eq!(
            state.process_chunk(&chunk(vec![0.1; 100], 24_000, false)),
            PlaybackAction::Audio { first: true }
        );
        assert_eq!(
            state.process_chunk(&chunk(vec![0.2; 100], 24_000, false)),
            PlaybackAction::Audio { first: false }
        );
        assert_eq!(
            state.process_chunk(&chunk(vec![], 0, true)),
            PlaybackAction::Final {
                was_speaking: true,
                abort: false
            }
        );
        assert_eq!(
            state.process_chunk(&chunk(vec![0.3; 100], 24_000, false)),
            PlaybackAction::Audio { first: true }
        );
    }

    #[test]
    fn shutdown_flag_stops_drain_loop() {
        let (tx, rx) = crossbeam_channel::bounded::<AudioChunkPayload>(4);
        let viseme = VisemeState::default();
        let shutdown = Arc::new(AtomicBool::new(false));
        let (cue_tx, _cue_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

        let drain_shutdown = Arc::clone(&shutdown);
        let drain_viseme = viseme.clone();
        let handle = std::thread::spawn(move || {
            drain_until_shutdown(&rx, &drain_viseme, &drain_shutdown, &cue_tx, Instant::now());
        });

        drop(tx.send(chunk(vec![0.1; 100], 24_000, false)));
        std::thread::sleep(Duration::from_millis(50));
        shutdown.store(true, Ordering::Relaxed);
        drop(handle.join());
    }

    #[test]
    fn bounded_channel_capacity() {
        const { assert!(PLAYBACK_CHANNEL_CAPACITY > 0) };
        const { assert!(PLAYBACK_CHANNEL_CAPACITY <= 256) };
    }
}
