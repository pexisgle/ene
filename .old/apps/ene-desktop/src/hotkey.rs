//! Global Alt+Space hotkey registration.
//!
//! `global-hotkey` supports Windows, macOS, and Linux X11; Wayland has
//! no global-shortcut backend, so the manager is `None` there and the
//! runtime falls back to in-window key handling.
//!
//! Registration is reconciled with `desktop.spotlight_enabled` every
//! frame, so toggling the setting registers / unregisters the OS grab
//! instead of leaving Alt+Space shadowed for the whole session.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState as EventState};

/// What [`HotkeyState::sync_enabled`] must do to reach the desired
/// registration state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySync {
    Register,
    Unregister,
    Noop,
}

/// Desired registration transition for a `(registered, enabled)` pair.
pub const fn hotkey_sync_needed(registered: bool, enabled: bool) -> HotkeySync {
    match (registered, enabled) {
        (false, true) => HotkeySync::Register,
        (true, false) => HotkeySync::Unregister,
        _ => HotkeySync::Noop,
    }
}

/// Whether the in-window Alt+Space fallback should stay active. The
/// global grab consumes the key wherever it fires, so both paths must
/// never be live at the same time.
pub const fn in_window_fallback_active(global_registered: bool) -> bool {
    !global_registered
}

/// Track a hotkey event against the held state; returns `true` only
/// for a fresh press (X11 auto-repeat emits `Pressed` per repeated
/// `KeyPress` while the key is held).
fn track_press(held: &mut bool, state: EventState) -> bool {
    match state {
        EventState::Pressed if !*held => {
            *held = true;
            true
        }
        EventState::Pressed => false,
        EventState::Released => {
            *held = false;
            false
        }
    }
}

pub struct HotkeyState {
    manager: Option<GlobalHotKeyManager>,
    hotkey: HotKey,
    registered: bool,
    /// Skip further `register` calls after the OS rejected the grab
    /// until Spotlight is toggled off. Otherwise a taken Alt+Space
    /// would warn on every frame.
    register_blocked: bool,
    /// `true` while the OS reports the key held (see [`track_press`]).
    held: bool,
}

impl HotkeyState {
    /// Attempt registration. Returns a state with no manager on
    /// platforms without a backend, and logs a warning when the
    /// registration is taken.
    pub fn new() -> Self {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Global hotkey manager unavailable; Alt+Space falls back to in-window handling"
                );
                return Self::unsupported();
            }
        };
        let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Space);
        let mut state = Self {
            manager: Some(manager),
            hotkey,
            registered: false,
            register_blocked: false,
            held: false,
        };
        if let Err(error) = state.register() {
            tracing::warn!(
                error = %error,
                "Failed to register Alt+Space; falls back to in-window handling"
            );
        }
        state
    }

    /// State for platforms without a global-hotkey backend.
    pub fn unsupported() -> Self {
        Self {
            manager: None,
            hotkey: HotKey::new(Some(Modifiers::ALT), Code::Space),
            registered: false,
            register_blocked: false,
            held: false,
        }
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }

    fn register(&mut self) -> Result<(), String> {
        let Some(manager) = self.manager.as_ref() else {
            return Ok(());
        };
        if self.register_blocked {
            return Ok(());
        }
        match manager.register(self.hotkey) {
            Ok(()) => {
                self.registered = true;
                Ok(())
            }
            Err(error) => {
                self.register_blocked = true;
                Err(error.to_string())
            }
        }
    }

    fn unregister(&mut self) -> Result<(), String> {
        self.register_blocked = false;
        if let Some(manager) = self.manager.as_ref() {
            manager.unregister(self.hotkey).map_err(|e| e.to_string())?;
        }
        self.registered = false;
        Ok(())
    }

    /// Reconcile OS registration with the `spotlight_enabled` setting.
    /// Idempotent: no OS call when the state already matches.
    pub fn sync_enabled(&mut self, enabled: bool) -> Result<(), String> {
        match hotkey_sync_needed(self.registered, enabled) {
            HotkeySync::Register => self.register(),
            HotkeySync::Unregister => self.unregister(),
            HotkeySync::Noop => Ok(()),
        }
    }

    /// Drain pending hotkey events; `true` on a fresh press of the
    /// registered shortcut. Auto-repeat presses while the key is held
    /// are coalesced into the initial press.
    pub fn consume_press(&mut self) -> bool {
        let mut pressed = false;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.hotkey.id() && track_press(&mut self.held, event.state) {
                pressed = true;
            }
        }
        pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_needed_covers_all_states() {
        assert_eq!(hotkey_sync_needed(false, true), HotkeySync::Register);
        assert_eq!(hotkey_sync_needed(true, false), HotkeySync::Unregister);
        assert_eq!(hotkey_sync_needed(false, false), HotkeySync::Noop);
        assert_eq!(hotkey_sync_needed(true, true), HotkeySync::Noop);
    }

    #[test]
    fn fallback_is_exclusive_with_global_grab() {
        assert!(in_window_fallback_active(false));
        assert!(!in_window_fallback_active(true));
    }

    #[test]
    fn repeat_presses_are_coalesced_until_release() {
        let mut held = false;
        assert!(track_press(&mut held, EventState::Pressed));
        assert!(!track_press(&mut held, EventState::Pressed));
        assert!(!track_press(&mut held, EventState::Pressed));
        assert!(!track_press(&mut held, EventState::Released));
        assert!(track_press(&mut held, EventState::Pressed));
    }

    #[test]
    fn unsupported_platform_sync_is_idempotent() {
        let mut state = HotkeyState::unsupported();
        assert!(!state.is_registered());
        assert!(state.sync_enabled(true).is_ok());
        assert!(!state.is_registered());
        assert!(state.sync_enabled(false).is_ok());
        assert!(!state.is_registered());
    }
}
