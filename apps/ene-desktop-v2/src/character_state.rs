//! PR2 stubs for character-state holders that the v2 settings UI
//! (PR2) writes into and the real character renderer (PR3/PR4)
//! consumes.
//!
//! What lives here:
//!
//! - [`AnimationControl`] — play/pause toggle for the currently-loaded
//!   VRMA. The legacy Bevy equivalent lives in `character.rs`; the v2
//!   renderer reads this struct every frame.
//! - [`EmotionCommand`] / [`EmotionQueue`] — pending expression
//!   changes. The AI bridge (PR1) and the manual-expression test
//!   buttons in `settings_ui::page_character` push commands; the
//!   v2 renderer (PR4.4) drains due commands once per frame and
//!   pushes the resulting weights into the loaded VRM model.
//! - [`ActiveEmotion`] — the most recently applied emotion, kept by
//!   [`CharacterRenderer`](crate::character::CharacterRenderer) so
//!   it can fade the weight back to neutral after the hold elapses.
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

/// One pending expression change pushed by either the AI bridge
/// (`AppEvent::EmoteToken`) or the settings UI's "Manual
/// Expressions" buttons. Commands that are not yet due
/// (`target_time > now_secs`) are kept in the queue and drained
/// the next time the renderer ticks.
#[derive(Clone, Debug)]
pub struct EmotionCommand {
    /// Expression name (e.g. `"happy"`, `"sad"`, `"blink_l"`).
    /// Matches the keys of the `VRMC_vrm.expressions.preset` /
    /// `custom` object in the source VRM.
    pub emotion: String,
    /// Absolute time (seconds since
    /// [`SettingsUi::started_at`](crate::settings_ui::SettingsUi::started_at))
    /// at which this command becomes active. Commands with a
    /// future `target_time` are re-queued.
    pub target_time: f64,
    /// How long the weight should stay at `weight` before
    /// fading back to zero.
    pub hold_secs: f64,
    /// Target weight in `[0, 1]`. The AI bridge always pushes
    /// `1.0`; the manual-expression buttons also push `1.0`
    /// for now. A future `expression` slider can override it.
    pub weight: f32,
}

#[derive(Default, Debug)]
pub struct EmotionQueue {
    pub commands: VecDeque<EmotionCommand>,
}

impl EmotionQueue {
    /// Append a command to the back of the queue. The runtime
    /// calls this from `AppEvent::EmoteToken` and from the
    /// settings UI.
    pub fn push(&mut self, command: EmotionCommand) {
        self.commands.push_back(command);
    }

    /// Pop all commands whose `target_time` is at or before
    /// `now_secs`. Commands scheduled for the future are kept
    /// in the queue in their original order. Used by the
    /// renderer once per frame.
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
/// renderer reads `hold_until_secs` to know when to start
/// fading the weight back to zero, and it overwrites
/// `name`/`weight` whenever a new command of a different
/// expression arrives.
///
/// PR4.4 only tracks **one** active emotion at a time (last
/// write wins on a name change). The legacy `bevy_vrm1`
/// supports blended stacks; layering that on top of the
/// `ExpressionLayer::apply_weights` API is a PR4.5+ task.
#[derive(Clone, Debug)]
pub struct ActiveEmotion {
    pub name: String,
    pub weight: f32,
    pub hold_until_secs: f64,
}

/// Pure transition logic used by
/// [`CharacterRenderer::apply_emotions`](crate::character::CharacterRenderer::apply_emotions).
/// Split out from the renderer so the weight-clearing
/// behaviour is unit-testable without a live `wgpu` device.
///
/// Given the commands that were just drained from the queue
/// (`drained`), the currently active emotion (`current`), and
/// the current time (`now_secs`), return:
///
/// - `new_active` — the renderer's next `active_emotion`
///   field. `None` means "no active emotion; nothing to fade".
/// - `updates` — a list of `(name, weight)` pairs that the
///   caller must apply to the model's `ExpressionLayer` via
///   [`ExpressionLayer::set_expression`](ene_vrm::ExpressionLayer::set_expression).
///   Order matters: when an old emotion is replaced, the
///   `(prev.name, 0.0)` update is emitted *before* the new
///   `(cmd.name, cmd.weight)` update so the renderer never
///   sees both at non-zero weight in the same frame.
///
/// **Why the explicit clear**: in PR4.4, `ExpressionLayer`
/// stores all emotion weights in a single `BTreeMap`. The
/// AI bridge and the manual-expression buttons push
/// `EmotionCommand { emotion, weight: 1.0, .. }`. If the
/// caller wrote the new weight without first zeroing the old
/// one, the old weight would persist — and since the
/// `ExpressionLayer::weights` map is the single source of
/// truth read by the GPU upload, the previous expression
/// would keep affecting the model until the fade logic
/// eventually brought it below `FADE_FLOOR`. Clicking
/// "happy" then "neutral" left "happy" at weight 1.0 and
/// produced the "every expression squints the eyes" bug
/// reported in the PR4.4 review.
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
        // Clear the previous active emotion's weight so the
        // new one starts at full strength. Look-at expressions
        // (PR4.5+) are not in `active_emotion` and therefore
        // are preserved.
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

    /// Regression test for the PR4.4 review bug: clicking
    /// "happy" then "neutral" left the "happy" weight at
    /// `1.0` in the model's `ExpressionLayer`, so the GPU
    /// kept squinting the eyes even though the active
    /// emotion was now "neutral". The fix is for
    /// `transition_emotions` to emit a `(prev.name, 0.0)`
    /// update *before* the `(new.name, weight)` update when a
    /// drained command of a different expression arrives.
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
        // active = "happy". The bug was that `updates` only
        // contained ("neutral", 1.0); the explicit clear of
        // ("happy", 0.0) is the fix.
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

        // Keep fading — the exact float values drift due to
        // rounding, so compare with epsilon instead of `==`.
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

        // Final fade drops below 0.01 → active becomes None and
        // the last update zeroes the weight. `0.011 * 0.9 =
        // 0.0099 < 0.01`.
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
