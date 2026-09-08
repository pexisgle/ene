//! Global hotkeys for stage window actions.

use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use thiserror::Error;

use super::ShellCommand;

/// Registration state for one hotkey, split out of [`HotkeyManager`] so the
/// retry lifecycle can be exercised without a display server.
struct HotkeyRegistration {
    ok: bool,
    retry_at: Instant,
}

impl HotkeyRegistration {
    fn initial(register: impl FnOnce() -> bool) -> Self {
        Self {
            ok: register(),
            retry_at: Instant::now(),
        }
    }

    fn retry(&mut self, register: impl FnOnce() -> bool) -> bool {
        if self.ok || self.retry_at.elapsed() < Duration::from_secs(2) {
            return false;
        }
        self.retry_at = Instant::now();
        self.ok = register();
        self.ok
    }
}

/// Registers Alt+Space and polls for global hotkey events.
pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    spotlight: HotKey,
    spotlight_state: HotkeyRegistration,
    spotlight_id: u32,
}

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("hotkey manager: {0}")]
    Manager(String),
    #[error("register {label}: {detail}")]
    Register { label: &'static str, detail: String },
}

impl HotkeyManager {
    /// Best-effort registration: skips hotkeys that are already taken.
    pub fn new() -> Result<Self, HotkeyError> {
        let manager =
            GlobalHotKeyManager::new().map_err(|err| HotkeyError::Manager(err.to_string()))?;
        let spotlight = HotKey::new(Some(Modifiers::ALT), Code::Space);
        let spotlight_id = spotlight.id();
        let spotlight_state = HotkeyRegistration::initial(|| manager.register(spotlight).is_ok());
        if !spotlight_state.ok {
            tracing::warn!("could not register Alt+Space hotkey; will retry");
        }
        Ok(Self {
            manager,
            spotlight,
            spotlight_state,
            spotlight_id,
        })
    }

    fn retry_spotlight(&mut self) {
        if !self.spotlight_state.ok
            && self
                .spotlight_state
                .retry(|| self.manager.register(self.spotlight).is_ok())
        {
            tracing::info!("registered Alt+Space hotkey on retry");
        }
    }

    pub fn poll(&mut self) -> Option<ShellCommand> {
        self.retry_spotlight();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if event.id == self.spotlight_id {
                return Some(ShellCommand::OpenSpotlight);
            }
        }
        None
    }

    /// Whether Alt+Space is currently registered, including successful retries.
    #[must_use]
    pub fn spotlight_active(&self) -> bool {
        self.spotlight_state.ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_initial_stays_inactive_until_a_late_retry_succeeds() {
        let mut state = HotkeyRegistration::initial(|| false);
        assert!(!state.ok);

        // Still inside the 2 s backoff window, so no attempt happens even
        // though registration would succeed now.
        state.retry(|| true);
        assert!(!state.ok);

        state.retry_at -= Duration::from_secs(3);
        state.retry(|| true);
        assert!(state.ok);
    }

    #[test]
    fn active_state_never_attempts_another_registration() {
        let mut state = HotkeyRegistration::initial(|| true);
        state.retry_at -= Duration::from_secs(3);
        let mut attempted = false;
        state.retry(|| {
            attempted = true;
            false
        });
        assert!(state.ok);
        assert!(!attempted);
    }
}
