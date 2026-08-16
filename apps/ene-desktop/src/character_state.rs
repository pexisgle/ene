//! State holders that the settings UI writes into and the
//! character renderer consumes. Provides [`AnimationControl`],
//! [`EmotionCommand`] / [`EmotionQueue`], and [`ActiveEmotion`]
//! — the data types only; the rendering and queueing
//! transitions live in `system::ui_consumers` and
//! `resource::emotion_pipeline`.
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct AnimationControl {
    pub playing: bool,
}

impl Default for AnimationControl {
    fn default() -> Self {
        Self::new()
    }
}

impl AnimationControl {
    pub const fn new() -> Self {
        Self { playing: true }
    }

    pub const fn toggle_playing(&mut self) {
        self.playing = !self.playing;
    }
}

/// One pending expression change pushed by the AI bridge or the
/// settings UI's manual-expression buttons. Commands with a
/// future `target_time` are kept in the queue and drained on the
/// next tick.
#[derive(Clone, Debug)]
pub struct EmotionCommand {
    /// Expression names include `"happy"`, `"sad"`, `"blink_l"`.
    pub emotion: String,
    /// Absolute time at which this command becomes active.
    pub target_time: f64,
    /// How long the weight should stay at `weight` before fading
    /// back to zero.
    pub hold_secs: f64,
    /// Target weight in `[0, 1]`.
    pub weight: f32,
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

/// Currently-applied emotion tracked by the renderer. The
/// renderer reads `hold_until_secs` to know when to start fading
/// the weight back to zero, and overwrites `name`/`weight`
/// whenever a new command of a different expression arrives.
///
/// Only one active emotion is tracked at a time (last write
/// wins on a name change).
#[derive(Clone, Debug)]
pub struct ActiveEmotion {
    pub name: String,
    /// Target weight while in the hold window.
    pub weight: f32,
    /// Wall-clock seconds at which the hold window ends and the
    /// fade-out begins.
    pub hold_until_secs: f64,
}
