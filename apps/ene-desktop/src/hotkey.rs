//! Global Alt+Space hotkey registration.
//!
//! `global-hotkey` supports Windows, macOS, and Linux X11. Wayland has
//! no global-shortcut backend, so `HotkeyState::try_new` returns `None`
//! there and the runtime falls back to in-window key handling.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState as EventState};

/// Registered global Alt+Space shortcut. Dropping the manager
/// unregisters the hotkey.
pub struct HotkeyState {
    // Keep-alive: the manager owns the OS registration.
    _manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

impl HotkeyState {
    /// Register Alt+Space. Returns `None` (with a warning logged) when
    /// the platform has no backend or the registration is taken.
    pub fn try_new() -> Option<Self> {
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Global hotkey manager unavailable; Alt+Space falls back to in-window handling"
                );
                return None;
            }
        };
        let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Space);
        if let Err(error) = manager.register(hotkey) {
            tracing::warn!(
                error = %error,
                "Failed to register Alt+Space; falls back to in-window handling"
            );
            return None;
        }
        Some(Self {
            _manager: manager,
            hotkey,
        })
    }

    /// Drain pending hotkey events; `true` when the registered shortcut
    /// was pressed since the last call.
    pub fn consume_press(&self) -> bool {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.hotkey.id() && event.state == EventState::Pressed {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_space_hotkey_identity_is_stable() {
        let a = HotKey::new(Some(Modifiers::ALT), Code::Space);
        let b = HotKey::new(Some(Modifiers::ALT), Code::Space);
        assert_eq!(a.id(), b.id());
        assert_eq!(a.key, Code::Space);
        assert!(a.mods.contains(Modifiers::ALT));
    }
}
