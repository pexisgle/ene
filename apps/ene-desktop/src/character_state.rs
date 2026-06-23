//! State holders that the v2 settings UI writes into and the
//! character renderer consumes. Provides [`AnimationControl`],
//! [`EmotionCommand`] / [`EmotionQueue`], and [`ActiveEmotion`]
//! — the data types only; the rendering and queueing
//! transitions now live in `system::ui_consumers` and
//! `resource::emotion_pipeline` (Phase 7.5).
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

/// One pending expression change pushed by the AI bridge or the
/// settings UI's manual-expression buttons. Commands with a
/// future `target_time` are kept in the queue and drained on the
/// next tick.
#[derive(Clone, Debug)]
pub struct EmotionCommand {
    /// Expression name (e.g. `"happy"`, `"sad"`, `"blink_l"`).
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub emotion: String,
    /// Absolute time at which this command becomes active.
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub target_time: f64,
    /// How long the weight should stay at `weight` before fading
    /// back to zero.
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub hold_secs: f64,
    /// Target weight in `[0, 1]`.
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub weight: f32,
}

#[derive(Default, Debug)]
pub struct EmotionQueue {
    pub commands: VecDeque<EmotionCommand>,
}

impl EmotionQueue {
    /// Append a command to the back of the queue.
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
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub name: String,
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub weight: f32,
    /// Phase 7.2 skeleton: read by the upcoming render-side
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub hold_until_secs: f64,
}
