//! Per-action UI dispatcher system.
//!
//! Consumes [`SettingsActionEvent`] messages (written by the
//! winit hotkey handlers via
//! [`crate::runtime::Runtime::dispatch_settings_action`])
//! and applies the matching arm.
//!
//! Phase 7.1 scope: the system is a **drain** that records
//! the per-frame action count for the runtime. The actual
//! per-action work continues to happen in
//! [`apply_action`](crate::settings_ui::widgets::apply_action)
//! called from `runtime.rs::dispatch_settings_action` so the
//! existing `&mut CharacterSettings` access path is
//! preserved.
//!
//! Future work (Phase 8) rewrites the page-render code
//! (`page_character.rs` etc.) to write
//! `SettingsActionEvent` messages instead of calling
//! `apply_action` directly, at which point the
//! `dispatch_settings_action` legacy call site is removed
//! and the dispatcher becomes the single source of truth.
//!
//! The runtime still writes one `SettingsActionEvent` per
//! hotkey press; this system drains the queue so a stale
//! event does not survive a `RedrawRequested` double-fire
//! (winit 0.30's behaviour on Windows).
use bevy_ecs::prelude::*;

use crate::event::ui_action::SettingsActionEvent;

/// Drain every `SettingsActionEvent` queued this frame.
///
/// The runtime's `dispatch_settings_action` writes one
/// `SettingsActionEvent` and then immediately calls the
/// legacy `apply_action` so the per-frame mutation lands on
/// `AppState::settings`. This system exists so the message
/// bus does not accumulate unread events.
pub fn apply_settings_action_system(mut events: MessageReader<SettingsActionEvent>) {
    for _ in events.read() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_runs_without_panicking() {
        let mut world = World::new();
        world.init_resource::<Messages<SettingsActionEvent>>();
        for _ in 0..3 {
            world.write_message(SettingsActionEvent {
                action: crate::settings_ui::widgets::SettingsAction::TogglePlay,
            });
        }
        let mut schedule = Schedule::default();
        schedule.add_systems(apply_settings_action_system);
        // The dispatcher must not panic when there are
        // pending events; the per-`MessageReader` cursor
        // is what gets drained, not the underlying
        // `Messages<T>` buffer.
        schedule.run(&mut world);
    }

    #[test]
    fn empty_queue_is_noop() {
        let mut world = World::new();
        world.init_resource::<Messages<SettingsActionEvent>>();
        let mut schedule = Schedule::default();
        schedule.add_systems(apply_settings_action_system);
        schedule.run(&mut world);
    }
}
