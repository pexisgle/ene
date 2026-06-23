//! Emotion pipeline state.
//!
//! The [`apply_emotions_system`](crate::system::ui_consumers::apply_emotions_system)
//! drains the per-entity [`UiEmotionQueue`](crate::component::ui::UiEmotionQueue)
//! into this resource every frame. The character-render path reads
//! the queue (and tracks the active emotion) instead of
//! `AppState`-side scratch state.
//!
//! Phase 7.5: replaces the legacy `PendingActions::emotion_commands`
//! buffer. Phase 7.2 will move the renderer call itself into a
//! `Last`-stage system that holds `NonSend<CharacterRenderer>` and
//! reads this resource directly.
use std::collections::VecDeque;

use bevy_ecs::prelude::*;

use crate::character_state::{ActiveEmotion, EmotionCommand};

#[derive(Resource, Debug, Default)]
pub struct EmotionPipelineState {
    /// Commands buffered by the AI bridge / settings UI. Drained by
    /// the render-side system in `AppSet::Render`.
    pub pending: VecDeque<EmotionCommand>,
    /// Last-applied emotion + remaining hold time. Read by the
    /// render-side system to drive fade-out after the hold window.
    /// Phase 7.2 skeleton: consumed by the upcoming
    /// `apply_emotions_render_system` body-migration pass.
    #[allow(
        dead_code,
        reason = "Phase 7.2 skeleton: consumed by the upcoming body-migration pass."
    )]
    pub active: Option<ActiveEmotion>,
}
