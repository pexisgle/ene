//! TTS audio playback via `rodio`.
//!
//! A dedicated OS thread owns the cpal output stream, rodio mixer, and
//! player (all must stay alive on the thread that created them). The AI
//! bridge pump forwards [`EneEvent::AudioChunk`](ene_runtime::EneEvent::AudioChunk)
//! payloads over a [`crossbeam_channel`]; the playback thread appends
//! each chunk to the sink, feeds the same PCM to the shared
//! [`VisemeState`](super::VisemeState) for lip-sync, and toggles the
//! `tts_playing` flag used for self-voice suppression.
//!
//! The playback thread exits when the channel closes **or** when the
//! shared shutdown flag is raised. The flag is necessary because the
//! channel sender is cloned into the AI bridge pump task, so dropping
//! the `AppState`'s own sender does not close the channel (H2).
//!
//! Gated behind the `voice` feature; without it the desktop builds a
//! text-only shell and this module is not compiled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rodio::Player;
use rodio::buffer::SamplesBuffer;
use rodio::mixer::{self, Mixer};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{AudioChunkPayload, AudioChunkReceiver, AudioChunkSender, VisemeState};

/// Manually assembled audio sink: a rodio [`Mixer`] fed into a cpal output
/// stream, with a [`Player`] for queueing sounds.
struct AudioSink {
    _mixer: Mixer,
    player: Player,
    _stream: cpal::Stream,
}

/// Open the default output device and set up a rodio mixer + player backed
/// by a raw cpal output stream.
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
/// the playback thread (L10). Bounds memory growth when the sink cannot
/// keep up; the pump drops the oldest chunk once the buffer is full.
pub(crate) const PLAYBACK_CHANNEL_CAPACITY: usize = 64;

/// Handle to the running playback thread.
///
/// Dropping the handle raises the shutdown flag and joins the thread.
/// The thread exits once the paired [`AudioChunkSender`] is dropped
/// (channel closed) **or** the shutdown flag is raised — whichever
/// comes first. The flag is required because a second sender clone
/// lives in the AI bridge pump task, so the channel may stay open even
/// after `AppState` drops its own sender (H2).
pub struct AudioPlaybackHandle {
    join: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl AudioPlaybackHandle {
    /// Stop playback and wait for the playback thread to exit.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
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
/// alive.
pub fn spawn_playback(
    viseme: VisemeState,
    tts_playing: Arc<AtomicBool>,
) -> (AudioChunkSender, AudioPlaybackHandle) {
    let (tx, rx) = crossbeam_channel::bounded::<AudioChunkPayload>(PLAYBACK_CHANNEL_CAPACITY);
    let shutdown = Arc::new(AtomicBool::new(false));
    let loop_shutdown = Arc::clone(&shutdown);
    let join = std::thread::Builder::new()
        .name("ene-audio-playback".to_string())
        .spawn(move || playback_loop(rx, viseme, tts_playing, loop_shutdown))
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
        // No audio backend: drain and discard so the sender never blocks.
        drain_until_shutdown(&rx, &viseme, &shutdown);
        return;
    };

    // Whether any non-final chunk has been appended for the current turn.
    let mut state = PlaybackState::new();

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
                } else if was_speaking {
                    sink.player.sleep_until_end();
                }
                tts_playing.store(false, Ordering::Relaxed);
                viseme.reset();
            }
            PlaybackAction::Audio { first } => {
                // Queue the PCM for time-aligned viseme analysis (M4). The
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

    // Channel closed or shutdown requested: stop any in-flight audio.
    tts_playing.store(false, Ordering::Relaxed);
    viseme.reset();
}

/// Drain-and-discard loop used when no audio output device is available.
fn drain_until_shutdown(rx: &AudioChunkReceiver, viseme: &VisemeState, shutdown: &Arc<AtomicBool>) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(SHUTDOWN_POLL_INTERVAL) {
            Ok(chunk) => {
                if chunk.is_final {
                    viseme.reset();
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
/// be unit-tested without a real audio device (M5).
struct PlaybackState {
    /// Whether any non-final chunk has been appended for the current turn.
    speaking: bool,
}

impl PlaybackState {
    fn new() -> Self {
        Self { speaking: false }
    }

    /// Process one chunk, returning whether `tts_playing` should be set
    /// and whether the viseme state should be reset.
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

/// Action the playback loop should take for a given chunk.
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
        }
    }

    fn final_chunk(abort: bool) -> AudioChunkPayload {
        AudioChunkPayload {
            pcm: Vec::new(),
            sample_rate: 0,
            is_final: true,
            abort,
        }
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
        // Start of utterance.
        assert_eq!(
            state.process_chunk(&chunk(vec![0.1; 100], 24_000, false)),
            PlaybackAction::Audio { first: true }
        );
        // Middle of utterance.
        assert_eq!(
            state.process_chunk(&chunk(vec![0.2; 100], 24_000, false)),
            PlaybackAction::Audio { first: false }
        );
        // End of utterance.
        assert_eq!(
            state.process_chunk(&chunk(vec![], 0, true)),
            PlaybackAction::Final {
                was_speaking: true,
                abort: false
            }
        );
        // Next utterance starts fresh.
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

        let drain_shutdown = Arc::clone(&shutdown);
        let drain_viseme = viseme.clone();
        let handle = std::thread::spawn(move || {
            drain_until_shutdown(&rx, &drain_viseme, &drain_shutdown);
        });

        // Send a chunk, then raise the shutdown flag.
        let _ = tx.send(chunk(vec![0.1; 100], 24_000, false));
        std::thread::sleep(Duration::from_millis(50));
        shutdown.store(true, Ordering::Relaxed);
        // The drain loop should exit within the poll interval.
        let _ = handle.join();
    }

    #[test]
    fn bounded_channel_capacity() {
        const { assert!(PLAYBACK_CHANNEL_CAPACITY > 0) };
        const { assert!(PLAYBACK_CHANNEL_CAPACITY <= 256) };
    }
}
