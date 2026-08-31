//! Window-level interaction lifecycle for the Stage overlay.
//!
//! `set_cursor_hittest` (and later platform input regions) has one
//! authority: this controller. Gesture classification stays in
//! [`crate::interaction::GestureTracker`]; body placement stays in
//! [`crate::drag::BodyDrag`].

use winit::window::Window;

/// Window-level interaction mode. Platform backends map this to OS hit-test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InteractionMode {
    /// Overlay does not receive pointer events (click-through).
    #[default]
    Passive,
    /// Overlay receives pointer events for UI / VRM hover.
    Interactive,
    /// A body drag is in progress; keep events even off-silhouette.
    Dragging,
    /// A text field or other focusable UI owns the keyboard.
    UiFocused,
}

/// Display-side request from overlay UI. Display-only chrome does not set this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiInteractionRequest {
    #[default]
    None,
    Interactive,
    Focus,
}

/// Why the controller returned to [`InteractionMode::Passive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    FocusLost,
    PointerLost,
    AvatarsChanged,
    WindowHidden,
    UiFocusReleased,
}

/// Inputs the controller needs besides its own remembered requests.
#[derive(Debug, Clone, Copy, Default)]
pub struct InteractionSnapshot {
    pub transparent: bool,
    pub click_through_preferred: bool,
    pub chrome_protected: bool,
    pub hovering_body: bool,
    pub dragging: bool,
    pub has_avatar: bool,
    pub window_visible: bool,
}

/// Single authority for overlay window hit-test.
#[derive(Debug, Clone)]
pub struct StageInteractionController {
    mode: InteractionMode,
    ui_request: UiInteractionRequest,
    snapshot: InteractionSnapshot,
    applied_hittest: Option<bool>,
}

impl Default for StageInteractionController {
    fn default() -> Self {
        Self {
            mode: InteractionMode::Passive,
            ui_request: UiInteractionRequest::None,
            snapshot: InteractionSnapshot::default(),
            applied_hittest: None,
        }
    }
}

impl StageInteractionController {
    #[must_use]
    pub const fn mode(&self) -> InteractionMode {
        self.mode
    }

    #[must_use]
    pub const fn ui_request(&self) -> UiInteractionRequest {
        self.ui_request
    }

    /// Whether the native window should receive pointer events.
    #[must_use]
    pub const fn cursor_hittest_enabled(&self) -> bool {
        match self.mode {
            InteractionMode::Passive => false,
            InteractionMode::Interactive
            | InteractionMode::Dragging
            | InteractionMode::UiFocused => true,
        }
    }

    pub fn request_ui(&mut self, request: UiInteractionRequest) {
        self.ui_request = request;
        self.recompute();
    }

    pub fn release_ui(&mut self) {
        self.ui_request = UiInteractionRequest::None;
        self.recompute();
    }

    pub fn sync(&mut self, snapshot: InteractionSnapshot) {
        self.snapshot = snapshot;
        self.recompute();
    }

    pub fn cancel(&mut self, reason: CancelReason) {
        match reason {
            CancelReason::FocusLost | CancelReason::PointerLost | CancelReason::UiFocusReleased => {
                self.ui_request = UiInteractionRequest::None;
                self.snapshot.dragging = false;
                self.snapshot.hovering_body = false;
                self.snapshot.chrome_protected = false;
            }
            CancelReason::AvatarsChanged => {
                self.snapshot.hovering_body = false;
                self.snapshot.dragging = false;
                self.snapshot.has_avatar = false;
            }
            CancelReason::WindowHidden => {
                self.ui_request = UiInteractionRequest::None;
                self.snapshot.dragging = false;
                self.snapshot.hovering_body = false;
                self.snapshot.chrome_protected = false;
                self.snapshot.window_visible = false;
            }
        }
        self.recompute();
    }

    /// Apply the current mode to the window. Returns whether the OS call ran.
    pub fn apply_to_window(&mut self, window: &Window) -> bool {
        let enabled = self.cursor_hittest_enabled();
        if self.applied_hittest == Some(enabled) {
            return false;
        }
        match window.set_cursor_hittest(enabled) {
            Ok(()) => {
                self.applied_hittest = Some(enabled);
                true
            }
            Err(err) => {
                tracing::debug!(error = %err, enabled, "cursor hittest unsupported");
                false
            }
        }
    }

    /// Test double: record the hit-test bit without a real window.
    #[cfg(test)]
    pub fn apply_mock(&mut self) -> bool {
        let enabled = self.cursor_hittest_enabled();
        if self.applied_hittest == Some(enabled) {
            return false;
        }
        self.applied_hittest = Some(enabled);
        true
    }

    #[cfg(test)]
    #[must_use]
    pub const fn applied_hittest(&self) -> Option<bool> {
        self.applied_hittest
    }

    fn recompute(&mut self) {
        if !self.snapshot.window_visible {
            self.mode = InteractionMode::Passive;
            return;
        }
        if !self.snapshot.transparent || !self.snapshot.click_through_preferred {
            self.mode = if self.ui_request == UiInteractionRequest::Focus {
                InteractionMode::UiFocused
            } else if self.snapshot.dragging {
                InteractionMode::Dragging
            } else {
                InteractionMode::Interactive
            };
            return;
        }
        let hovering_body = self.snapshot.hovering_body && self.snapshot.has_avatar;
        self.mode = if self.ui_request == UiInteractionRequest::Focus {
            InteractionMode::UiFocused
        } else if self.snapshot.dragging {
            InteractionMode::Dragging
        } else if self.ui_request == UiInteractionRequest::Interactive
            || self.snapshot.chrome_protected
            || hovering_body
        {
            InteractionMode::Interactive
        } else {
            InteractionMode::Passive
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transparent_click_through() -> InteractionSnapshot {
        InteractionSnapshot {
            transparent: true,
            click_through_preferred: true,
            window_visible: true,
            has_avatar: true,
            ..InteractionSnapshot::default()
        }
    }

    #[test]
    fn opaque_overlay_stays_interactive() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(InteractionSnapshot {
            transparent: false,
            click_through_preferred: true,
            window_visible: true,
            has_avatar: true,
            ..InteractionSnapshot::default()
        });
        assert_eq!(ctl.mode(), InteractionMode::Interactive);
        assert!(ctl.cursor_hittest_enabled());
    }

    #[test]
    fn preferred_click_through_starts_passive() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        assert_eq!(ctl.mode(), InteractionMode::Passive);
        assert!(!ctl.cursor_hittest_enabled());
    }

    #[test]
    fn clickable_ui_requests_interactive_entry() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        ctl.request_ui(UiInteractionRequest::Interactive);
        assert_eq!(ctl.mode(), InteractionMode::Interactive);
        assert!(ctl.cursor_hittest_enabled());
    }

    #[test]
    fn display_only_ui_does_not_enter_interactive() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        ctl.request_ui(UiInteractionRequest::None);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
    }

    #[test]
    fn body_hover_opens_interactive() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.hovering_body = true;
        ctl.sync(snap);
        assert_eq!(ctl.mode(), InteractionMode::Interactive);
    }

    #[test]
    fn dragging_keeps_events_off_silhouette() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.dragging = true;
        snap.hovering_body = false;
        ctl.sync(snap);
        assert_eq!(ctl.mode(), InteractionMode::Dragging);
        assert!(ctl.cursor_hittest_enabled());
    }

    #[test]
    fn focus_lost_clears_drag() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.dragging = true;
        ctl.sync(snap);
        ctl.cancel(CancelReason::FocusLost);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
        assert!(!ctl.snapshot.dragging);
    }

    #[test]
    fn avatar_reload_clears_stale_hover_and_drag() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.hovering_body = true;
        snap.dragging = true;
        ctl.sync(snap);
        ctl.cancel(CancelReason::AvatarsChanged);
        assert!(!ctl.snapshot.hovering_body);
        assert!(!ctl.snapshot.dragging);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
    }

    #[test]
    fn window_hide_returns_passive() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        ctl.request_ui(UiInteractionRequest::Focus);
        assert_eq!(ctl.mode(), InteractionMode::UiFocused);
        ctl.cancel(CancelReason::WindowHidden);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
        assert!(!ctl.cursor_hittest_enabled());
    }

    #[test]
    fn ui_focus_release_returns_passive() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        ctl.request_ui(UiInteractionRequest::Focus);
        ctl.cancel(CancelReason::UiFocusReleased);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
    }

    #[test]
    fn pointer_lost_clears_drag() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.dragging = true;
        ctl.sync(snap);
        ctl.cancel(CancelReason::PointerLost);
        assert_eq!(ctl.mode(), InteractionMode::Passive);
    }

    #[test]
    fn apply_mock_is_the_only_hittest_mutation() {
        let mut ctl = StageInteractionController::default();
        ctl.sync(transparent_click_through());
        assert!(ctl.apply_mock());
        assert_eq!(ctl.applied_hittest(), Some(false));
        assert!(!ctl.apply_mock());
        ctl.request_ui(UiInteractionRequest::Interactive);
        assert!(ctl.apply_mock());
        assert_eq!(ctl.applied_hittest(), Some(true));
    }

    #[test]
    fn chrome_protection_requests_interactive() {
        let mut ctl = StageInteractionController::default();
        let mut snap = transparent_click_through();
        snap.chrome_protected = true;
        ctl.sync(snap);
        assert_eq!(ctl.mode(), InteractionMode::Interactive);
    }
}
