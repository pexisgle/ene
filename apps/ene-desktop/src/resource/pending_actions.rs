//! Bridge resource between `bevy_ecs` systems and the legacy
//! `Runtime::about_to_wait` body.
//!
//! Phase 2 keeps most of the per-frame work in the legacy winit
//! handler because migrating it all to systems would mix in
//! Character / UI / Platform plugin concerns. Instead, the
//! `pump_legacy_events` system drains the [`crate::events::AppEvent`]
//! bus and writes the resulting *actions* into this resource; the
//! runtime then reads [`PendingActions`] at the end of the frame and
//! applies the changes to the legacy state.
use std::collections::VecDeque;

use bevy_ecs::prelude::*;

use crate::character_state::EmotionCommand;
use crate::settings::{PendingPermission, PendingUserInput};
use crate::settings_ui::PageKind;

#[derive(Resource, Debug, Default)]
pub struct PendingActions {
    /// `true` if the user requested shutdown (tray menu, window close,
    /// or AI bridge). The runtime calls `event_loop.exit()`.
    pub quit: bool,
    /// If `Some(_)`, open (and focus) the settings window on the
    /// given page. `None` toggles visibility using the current
    /// `UiState::settings_window_visible` value at the call site.
    pub open_settings: Option<Option<PageKind>>,
    /// Pending text to append to `UiState::ai_latest_response`.
    pub ai_text_deltas: Vec<String>,
    /// Pending permission request surfaced by the AI stream. The
    /// runtime copies this into `UiState::pending_permission`.
    pub ai_permission: Option<PendingPermission>,
    /// Pending user-input request surfaced by the AI stream. The
    /// runtime copies this into `UiState::pending_user_input`.
    pub ai_user_input: Option<PendingUserInput>,
    /// Drafts to allocate when a user-input request first appears.
    pub ai_user_input_drafts: usize,
    /// Pending emotion commands to enqueue for the next frame.
    pub emotion_commands: VecDeque<EmotionCommand>,
    /// Tick the GTK pump on Linux. Set when the tray subsystem is
    /// active.
    pub tick_gtk: bool,
}

impl PendingActions {
    /// Resets the per-frame buffer at the end of the frame.
    pub fn clear(&mut self) {
        self.quit = false;
        self.open_settings = None;
        self.ai_text_deltas.clear();
        self.ai_permission = None;
        self.ai_user_input = None;
        self.ai_user_input_drafts = 0;
        self.emotion_commands.clear();
        self.tick_gtk = false;
    }
}
