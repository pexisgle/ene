//! Pointer gesture classification for direct avatar interaction.

use std::time::{Duration, Instant};

use glam::Vec2;

/// Movement beyond this distance turns a press into a drag.
pub const DRAG_THRESHOLD_PX: f32 = 8.0;
/// A stationary press held for this long is a long press.
pub const LONG_PRESS: Duration = Duration::from_millis(600);
/// Two stationary presses on one avatar in this interval form a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// Agent handoff is suppressed inside this interval.
pub const REACTION_RATE_LIMIT: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveResult {
    None,
    DragStarted,
    Dragging,
}

impl MoveResult {
    #[must_use]
    pub const fn is_dragging(self) -> bool {
        matches!(self, Self::DragStarted | Self::Dragging)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionKind {
    Click,
    DoubleClick,
    LongPress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndResult {
    None,
    Dragged {
        soul_id: String,
    },
    Reaction {
        soul_id: String,
        kind: ReactionKind,
        rate_limited: bool,
    },
}

#[derive(Debug, Clone)]
struct ActiveGesture {
    pointer: PointerKind,
    id: u64,
    soul_id: String,
    start: Vec2,
    started_at: Instant,
    dragging: bool,
}

#[derive(Debug, Default)]
pub struct GestureTracker {
    active: Option<ActiveGesture>,
    last_click: Option<(String, Instant)>,
    last_reaction: Option<Instant>,
}

impl GestureTracker {
    #[must_use]
    pub fn press(
        &mut self,
        pointer: PointerKind,
        id: u64,
        position: Vec2,
        soul_id: Option<&str>,
        now: Instant,
    ) -> bool {
        if self.active.is_some() {
            return false;
        }
        let Some(soul_id) = soul_id else {
            self.last_click = None;
            return false;
        };
        self.active = Some(ActiveGesture {
            pointer,
            id,
            soul_id: soul_id.to_owned(),
            start: position,
            started_at: now,
            dragging: false,
        });
        true
    }

    #[must_use]
    pub fn move_to(&mut self, pointer: PointerKind, id: u64, position: Vec2) -> MoveResult {
        let Some(active) = self.active.as_mut() else {
            return MoveResult::None;
        };
        if active.pointer != pointer || active.id != id {
            return MoveResult::None;
        }
        if active.dragging {
            return MoveResult::Dragging;
        }
        if position.distance_squared(active.start) >= DRAG_THRESHOLD_PX.powi(2) {
            active.dragging = true;
            self.last_click = None;
            MoveResult::DragStarted
        } else {
            MoveResult::None
        }
    }

    #[must_use]
    pub fn release(&mut self, pointer: PointerKind, id: u64, now: Instant) -> EndResult {
        let Some(active) = self.active.take() else {
            return EndResult::None;
        };
        if active.pointer != pointer || active.id != id {
            self.active = Some(active);
            return EndResult::None;
        }
        if active.dragging {
            self.last_click = None;
            return EndResult::Dragged {
                soul_id: active.soul_id,
            };
        }

        let elapsed = now.saturating_duration_since(active.started_at);
        let kind = if elapsed >= LONG_PRESS {
            self.last_click = None;
            ReactionKind::LongPress
        } else if self.last_click.as_ref().is_some_and(|(soul_id, at)| {
            soul_id == &active.soul_id && now.saturating_duration_since(*at) <= DOUBLE_CLICK
        }) {
            self.last_click = None;
            ReactionKind::DoubleClick
        } else {
            self.last_click = Some((active.soul_id.clone(), now));
            ReactionKind::Click
        };
        let rate_limited = self
            .last_reaction
            .is_some_and(|at| now.saturating_duration_since(at) < REACTION_RATE_LIMIT);
        if !rate_limited {
            self.last_reaction = Some(now);
        }
        EndResult::Reaction {
            soul_id: active.soul_id,
            kind,
            rate_limited,
        }
    }

    pub fn cancel(&mut self, pointer: PointerKind, id: u64) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.pointer == pointer && active.id == id)
        {
            self.active = None;
            self.last_click = None;
            return true;
        }
        false
    }

    pub fn cancel_all(&mut self) {
        self.active = None;
        self.last_click = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn stationary_press_is_a_click() {
        let now = Instant::now();
        let mut tracker = GestureTracker::default();
        assert!(tracker.press(PointerKind::Mouse, 0, position(10.0, 10.0), Some("a"), now));
        assert_eq!(
            tracker.release(PointerKind::Mouse, 0, now + Duration::from_millis(100)),
            EndResult::Reaction {
                soul_id: "a".to_owned(),
                kind: ReactionKind::Click,
                rate_limited: false,
            }
        );
    }

    #[test]
    fn movement_past_threshold_is_a_drag() {
        let now = Instant::now();
        let mut tracker = GestureTracker::default();
        assert!(tracker.press(PointerKind::Touch, 7, Vec2::ZERO, Some("a"), now));
        assert_eq!(
            tracker.move_to(PointerKind::Touch, 7, position(DRAG_THRESHOLD_PX, 0.0)),
            MoveResult::DragStarted
        );
        assert_eq!(
            tracker.release(PointerKind::Touch, 7, now + Duration::from_millis(50)),
            EndResult::Dragged {
                soul_id: "a".to_owned()
            }
        );
    }

    #[test]
    fn long_press_and_double_click_are_distinct() {
        let now = Instant::now();
        let mut tracker = GestureTracker::default();
        assert!(tracker.press(PointerKind::Mouse, 0, Vec2::ZERO, Some("a"), now));
        assert!(matches!(
            tracker.release(PointerKind::Mouse, 0, now + LONG_PRESS),
            EndResult::Reaction {
                kind: ReactionKind::LongPress,
                ..
            }
        ));
        assert!(tracker.press(
            PointerKind::Mouse,
            0,
            Vec2::ZERO,
            Some("a"),
            now + Duration::from_secs(2)
        ));
        assert!(matches!(
            tracker.release(
                PointerKind::Mouse,
                0,
                now + Duration::from_secs(2) + Duration::from_millis(50)
            ),
            EndResult::Reaction {
                kind: ReactionKind::Click,
                ..
            }
        ));
        assert!(tracker.press(
            PointerKind::Mouse,
            0,
            Vec2::ZERO,
            Some("a"),
            now + Duration::from_secs(2) + Duration::from_millis(100)
        ));
        assert!(matches!(
            tracker.release(
                PointerKind::Mouse,
                0,
                now + Duration::from_secs(2) + Duration::from_millis(150)
            ),
            EndResult::Reaction {
                kind: ReactionKind::DoubleClick,
                rate_limited: true,
                ..
            }
        ));
    }

    #[test]
    fn rapid_click_on_another_avatar_is_rate_limited() {
        let now = Instant::now();
        let mut tracker = GestureTracker::default();
        assert!(tracker.press(PointerKind::Pen, 3, Vec2::ZERO, Some("a"), now));
        assert!(matches!(
            tracker.release(PointerKind::Pen, 3, now),
            EndResult::Reaction { .. }
        ));
        assert!(tracker.press(
            PointerKind::Pen,
            3,
            Vec2::ZERO,
            Some("b"),
            now + Duration::from_millis(100)
        ));
        assert!(matches!(
            tracker.release(PointerKind::Pen, 3, now + Duration::from_millis(100)),
            EndResult::Reaction {
                soul_id,
                rate_limited: true,
                ..
            } if soul_id == "b"
        ));
    }

    #[test]
    fn background_press_does_not_start_a_gesture() {
        let mut tracker = GestureTracker::default();
        assert!(!tracker.press(PointerKind::Mouse, 0, Vec2::ZERO, None, Instant::now()));
        assert_eq!(
            tracker.release(PointerKind::Mouse, 0, Instant::now()),
            EndResult::None
        );
    }
}
