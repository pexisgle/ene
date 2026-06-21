//! State holders that the v2 settings UI writes into and the
//! character renderer consumes. Provides [`AnimationControl`],
//! [`EmotionCommand`] / [`EmotionQueue`], and [`ActiveEmotion`].
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
    /// Append a command to the back of the queue.
    pub fn push(&mut self, command: EmotionCommand) {
        self.commands.push_back(command);
    }

    /// Pop all commands whose `target_time <= now_secs`. Future
    /// commands are re-queued in their original order.
    pub fn drain_due(&mut self, now_secs: f64) -> Vec<EmotionCommand> {
        let pending = std::mem::take(&mut self.commands);
        let mut due = Vec::new();
        let mut remaining = VecDeque::new();
        for cmd in pending {
            if cmd.target_time <= now_secs {
                due.push(cmd);
            } else {
                remaining.push_back(cmd);
            }
        }
        self.commands = remaining;
        due
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
    pub weight: f32,
    pub hold_until_secs: f64,
}

/// Pure transition logic used by
/// [`CharacterRenderer::apply_emotions`](crate::character::CharacterRenderer::apply_emotions).
/// Returns `new_active` and the `(name, weight)` pairs the caller
/// must apply to the model's `ExpressionLayer`. When an old
/// emotion is replaced the `(prev.name, 0.0)` update is emitted
/// before the new one so the renderer never sees both at
/// non-zero weight in the same frame.
pub fn transition_emotions(
    drained: &[EmotionCommand],
    current: Option<&ActiveEmotion>,
    now_secs: f64,
    fade_rate: f32,
    fade_floor: f32,
) -> (Option<ActiveEmotion>, Vec<(String, f32)>) {
    let mut updates: Vec<(String, f32)> = Vec::new();
    let mut new_active: Option<ActiveEmotion> = current.cloned();

    if let Some(cmd) = drained.last() {
        // Clear the previous weight before writing the new one:
        // `ExpressionLayer` is a single map read by the GPU
        // upload, so the old weight would otherwise persist
        // until the fade brought it below `fade_floor`. Look-at
        // expressions are not in `active_emotion` and are
        // preserved.
        if let Some(prev) = new_active.take() {
            updates.push((prev.name, 0.0));
        }
        updates.push((cmd.emotion.clone(), cmd.weight));
        new_active = Some(ActiveEmotion {
            name: cmd.emotion.clone(),
            weight: cmd.weight,
            hold_until_secs: now_secs + cmd.hold_secs,
        });
    }

    if let Some(active) = new_active.clone()
        && now_secs > active.hold_until_secs
    {
        let faded = (active.weight * fade_rate).max(0.0);
        if faded < fade_floor {
            updates.push((active.name, 0.0));
            new_active = None;
        } else {
            updates.push((active.name.clone(), faded));
            new_active = Some(ActiveEmotion {
                weight: faded,
                ..active
            });
        }
    }

    (new_active, updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(emotion: &str, target_time: f64, hold_secs: f64) -> EmotionCommand {
        EmotionCommand {
            emotion: emotion.to_string(),
            target_time,
            hold_secs,
            weight: 1.0,
        }
    }

    #[test]
    fn drain_due_returns_due_and_keeps_future() {
        let mut q = EmotionQueue::default();
        q.push(cmd("a", 0.0, 1.0));
        q.push(cmd("b", 5.0, 1.0));
        q.push(cmd("c", 1.0, 1.0));
        q.push(cmd("d", 10.0, 1.0));

        let due = q.drain_due(2.0);
        assert_eq!(
            due.iter().map(|c| c.emotion.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
        let remaining: Vec<String> = q.commands.iter().map(|c| c.emotion.clone()).collect();
        assert_eq!(remaining, vec!["b".to_string(), "d".to_string()]);
    }

    #[test]
    fn drain_due_at_now_zero_drains_only_immediate() {
        let mut q = EmotionQueue::default();
        q.push(cmd("a", 0.0, 1.0));
        q.push(cmd("b", -1.0, 1.0));
        q.push(cmd("c", 0.5, 1.0));
        let due = q.drain_due(0.0);
        assert_eq!(
            due.iter().map(|c| c.emotion.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(q.commands.len(), 1);
        assert_eq!(q.commands[0].emotion, "c");
    }

    #[test]
    fn drain_due_empty_queue_returns_empty() {
        let mut q = EmotionQueue::default();
        assert!(q.drain_due(100.0).is_empty());
        assert!(q.commands.is_empty());
    }

    #[test]
    fn drain_due_preserves_remaining_order() {
        let mut q = EmotionQueue::default();
        for (i, t) in [1.0_f64, 3.0, 2.0, 5.0, 4.0].iter().enumerate() {
            q.push(cmd(&format!("e{i}"), *t, 1.0));
        }
        // At t=2.5, e0 (1.0) and e2 (2.0) are due; the rest are
        // kept in their original input order.
        let due = q.drain_due(2.5);
        assert_eq!(
            due.iter().map(|c| c.emotion.as_str()).collect::<Vec<_>>(),
            vec!["e0", "e2"]
        );
        let names: Vec<String> = q.commands.iter().map(|c| c.emotion.clone()).collect();
        assert_eq!(
            names,
            vec!["e1".to_string(), "e3".to_string(), "e4".to_string()]
        );
    }

    /// Regression: clicking "happy" then "neutral" must zero
    /// the "happy" weight before applying "neutral", otherwise
    /// the GPU keeps squinting the eyes.
    #[test]
    fn transition_emotions_clears_previous_when_switching() {
        let mut q = EmotionQueue::default();
        q.push(cmd("happy", 0.0, 4.0));
        q.push(cmd("neutral", 0.5, 4.0));
        let drained = q.drain_due(1.0);

        // First transition: drained = [happy], no current.
        let (active1, updates1) = transition_emotions(&drained[..1], None, 1.0, 0.9, 0.01);
        assert_eq!(active1.as_ref().map(|a| a.name.as_str()), Some("happy"));
        assert_eq!(updates1, vec![("happy".to_string(), 1.0)]);

        // Second transition: drained = [neutral], previous
        // active = "happy". The clear ("happy", 0.0) must
        // come before the new ("neutral", 1.0).
        let (active2, updates2) =
            transition_emotions(&drained[1..], active1.as_ref(), 1.0, 0.9, 0.01);
        assert_eq!(active2.as_ref().map(|a| a.name.as_str()), Some("neutral"));
        assert_eq!(
            updates2,
            vec![("happy".to_string(), 0.0), ("neutral".to_string(), 1.0)],
        );
    }

    #[test]
    fn transition_emotions_fade_after_hold() {
        let active = ActiveEmotion {
            name: "sad".to_string(),
            weight: 1.0,
            hold_until_secs: 5.0,
        };
        // now=6.0 (past hold) → fade once.
        let (active2, updates) = transition_emotions(&[], Some(&active), 6.0, 0.9, 0.01);
        assert_eq!(active2.as_ref().map(|a| a.name.as_str()), Some("sad"));
        assert!((active2.as_ref().unwrap().weight - 0.9).abs() < 1e-6);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "sad");
        assert!((updates[0].1 - 0.9).abs() < 1e-6);

        // Keep fading — exact float values drift, so compare
        // with epsilon rather than `==`.
        let mut cur = active2.unwrap();
        for _ in 0..40 {
            if cur.weight < 0.01 {
                break;
            }
            let (next, updates) =
                transition_emotions(&[], Some(&cur), cur.hold_until_secs + 1.0, 0.9, 0.01);
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].0, "sad");
            assert!((updates[0].1 - cur.weight * 0.9).abs() < 1e-5);
            cur = next.unwrap_or_else(|| ActiveEmotion {
                name: "sad".to_string(),
                weight: 0.0,
                hold_until_secs: cur.hold_until_secs,
            });
        }

        // Final fade drops below 0.01 → active becomes None
        // and the last update zeroes the weight.
        // `0.011 * 0.9 = 0.0099 < 0.01`.
        let tiny = ActiveEmotion {
            name: "sad".to_string(),
            weight: 0.011,
            hold_until_secs: 100.0,
        };
        let (final_active, final_updates) = transition_emotions(&[], Some(&tiny), 101.0, 0.9, 0.01);
        assert!(final_active.is_none());
        assert_eq!(final_updates, vec![("sad".to_string(), 0.0)]);
    }

    #[test]
    fn transition_emotions_during_hold_is_noop() {
        let active = ActiveEmotion {
            name: "angry".to_string(),
            weight: 1.0,
            hold_until_secs: 10.0,
        };
        // now=3.0 (well within hold), no new commands.
        let (new_active, updates) = transition_emotions(&[], Some(&active), 3.0, 0.9, 0.01);
        assert_eq!(new_active.as_ref().map(|a| a.name.as_str()), Some("angry"));
        assert!(updates.is_empty());
    }

    #[test]
    fn transition_emotions_empty_with_no_active_is_noop() {
        let (new_active, updates) = transition_emotions(&[], None, 1.0, 0.9, 0.01);
        assert!(new_active.is_none());
        assert!(updates.is_empty());
    }
}
