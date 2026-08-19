//! Global hotkeys for stage window actions.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use thiserror::Error;

/// Window-level actions triggered by global hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAction {
    OpenSpotlight,
    OpenDetail,
    OpenLog,
    FocusChat,
    ToggleMic,
    Quit,
}

/// Registers Alt+Space, F1, F2, F4 and polls for events.
pub struct HotkeyManager {
    #[expect(dead_code, reason = "must outlive registered hotkeys")]
    manager: GlobalHotKeyManager,
    spotlight_id: u32,
    detail_id: u32,
    chat_id: u32,
    log_id: u32,
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
        let detail = HotKey::new(None, Code::F1);
        let chat = HotKey::new(None, Code::F2);
        let log = HotKey::new(None, Code::F4);
        let spotlight_id = spotlight.id();
        let detail_id = detail.id();
        let chat_id = chat.id();
        let log_id = log.id();
        if let Err(err) = manager.register(spotlight) {
            tracing::warn!(error = %err, "could not register Alt+Space hotkey");
        }
        if let Err(err) = manager.register(detail) {
            tracing::warn!(error = %err, "could not register F1 hotkey");
        }
        if let Err(err) = manager.register(chat) {
            tracing::warn!(error = %err, "could not register F2 hotkey");
        }
        if let Err(err) = manager.register(log) {
            tracing::warn!(error = %err, "could not register F4 hotkey");
        }
        Ok(Self {
            manager,
            spotlight_id,
            detail_id,
            chat_id,
            log_id,
        })
    }

    pub fn poll(&self) -> Option<ShellAction> {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if event.id == self.spotlight_id {
                return Some(ShellAction::OpenSpotlight);
            }
            if event.id == self.detail_id {
                return Some(ShellAction::OpenDetail);
            }
            if event.id == self.chat_id {
                return Some(ShellAction::FocusChat);
            }
            if event.id == self.log_id {
                return Some(ShellAction::OpenLog);
            }
        }
        None
    }
}
