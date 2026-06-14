//! PR2 stubs for character-state holders that the v2 settings UI
//! (PR2) writes into but the real character renderer (PR3/PR4) will
//! consume.
//!
//! These are intentionally minimal: they exist so the page-character
//! and page-graphics surfaces can compile and ship a working
//! settings UI before the renderer is in place.
//!
//! What lives here:
//!
//! - [`AnimationControl`] — play/pause toggle for the currently-loaded
//!   VRMA. The legacy Bevy equivalent lives in `character.rs`; the v2
//!   renderer (PR4) will read this struct every frame.
//! - [`EmotionCommand`] / [`EmotionQueue`] — pending expression
//!   changes. The legacy uses a `Vec<EmotionCommand>` on a Bevy
//!   resource; v2 keeps the same shape, and the renderer (PR4) will
//!   pop commands as their `target_time` elapses.
use std::collections::VecDeque;

#[derive(Clone, Debug, Default)]
pub struct AnimationControl {
    pub playing: bool,
}

impl AnimationControl {
    pub fn new() -> Self {
        Self { playing: true }
    }

    pub fn toggle_playing(&mut self) {
        self.playing = !self.playing;
    }
}

#[derive(Clone, Debug)]
#[expect(dead_code)] // PR2 writes these via the 6 expression buttons; PR4 will pop.
pub struct EmotionCommand {
    pub emotion: String,
    pub target_time: f64,
    pub hold_secs: f64,
}

#[derive(Default, Debug)]
pub struct EmotionQueue {
    pub commands: VecDeque<EmotionCommand>,
}

impl EmotionQueue {
    pub fn push(&mut self, command: EmotionCommand) {
        self.commands.push_back(command);
    }
}
