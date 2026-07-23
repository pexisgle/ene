//! TTS audio playback via `rodio`.
//!
//! A dedicated OS thread owns the rodio [`OutputStream`] and [`Sink`]
//! (both must stay alive on the thread that created them). The AI
//! bridge pump forwards [`EneEvent::AudioChunk`](ene_runtime::EneEvent::AudioChunk)
//! payloads over a [`crossbeam_channel`]; the playback thread appends
//! each chunk to the sink, feeds the same PCM to the shared
//! [`VisemeState`](super::VisemeState) for lip-sync, and toggles the
//! `tts_playing` flag used for self-voice suppression.
//!
//! Gated behind the `voice` feature; without it the desktop builds a
//! text-only shell and this module is not compiled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, Sink};

use super::{AudioChunkPayload, AudioChunkReceiver, AudioChunkSender, VisemeState};

/// Handle to the running playback thread.
///
/// Dropping the handle joins the thread. The thread exits once the
/// paired [`AudioChunkSender`] is dropped (channel closed), which
/// happens when the AI bridge pump goes away.
pub struct AudioPlaybackHandle {
    join: Option<JoinHandle<()>>,
}

impl AudioPlaybackHandle {
    /// Stop playback and wait for the playback thread to exit.
    pub fn stop(&mut self) {
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
    let (tx, rx) = crossbeam_channel::unbounded::<AudioChunkPayload>();
    let join = std::thread::Builder::new()
        .name("ene-audio-playback".to_string())
        .spawn(move || playback_loop(rx, viseme, tts_playing))
        .ok();
    (tx, AudioPlaybackHandle { join })
}

/// The playback thread body.
///
/// Opens the default output device once, then drains the channel until
/// it closes. If no output device is available the loop still drains
/// the channel (discarding audio) so the sender never blocks, but logs
/// a warning and leaves `tts_playing` untouched.
fn playback_loop(rx: AudioChunkReceiver, viseme: VisemeState, tts_playing: Arc<AtomicBool>) {
    let stream = match OutputStream::try_default() {
        Ok((stream, handle)) => match Sink::try_new(&handle) {
            Ok(sink) => Some((stream, sink)),
            Err(e) => {
                tracing::warn!(
                    component = "AudioPlayback",
                    error = %e,
                    "failed to create rodio sink; TTS audio disabled"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                component = "AudioPlayback",
                error = %e,
                "no default audio output device; TTS audio disabled"
            );
            None
        }
    };

    let Some((_stream, sink)) = stream else {
        // No audio backend: drain and discard so the sender never blocks.
        for chunk in rx {
            if chunk.is_final {
                viseme.reset();
            }
        }
        return;
    };

    // Whether any non-final chunk has been appended for the current turn.
    let mut speaking = false;

    for chunk in rx {
        if chunk.is_final {
            // Final marker: wait for queued audio to finish, then release
            // the self-voice suppression flag and reset lip-sync.
            if speaking {
                sink.sleep_until_end();
                speaking = false;
            }
            tts_playing.store(false, Ordering::Relaxed);
            viseme.reset();
            continue;
        }

        if chunk.pcm.is_empty() || chunk.sample_rate == 0 {
            continue;
        }

        // Feed lip-sync before queueing so the first audible frame already
        // has viseme data available to the render loop.
        viseme.feed_pcm(&chunk.pcm, chunk.sample_rate);

        let buffer = SamplesBuffer::new(1, chunk.sample_rate, chunk.pcm);
        sink.append(buffer);

        if !speaking {
            speaking = true;
            tts_playing.store(true, Ordering::Relaxed);
        }
    }

    // Channel closed (bridge dropped): stop any in-flight audio.
    tts_playing.store(false, Ordering::Relaxed);
    viseme.reset();
}
