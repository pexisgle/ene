//! Global hotkeys for stage window actions.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use thiserror::Error;

/// Window-level actions triggered by global hotkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAction {
    OpenSpotlight,
    OpenDetail,
    FocusChat,
    ToggleMic,
    Quit,
}

/// Registers Alt+Space, F1, and F2 and polls for events.
pub struct HotkeyManager {
    #[expect(dead_code, reason = "must outlive registered hotkeys")]
    manager: GlobalHotKeyManager,
    spotlight_id: u32,
    detail_id: u32,
    chat_id: u32,
}

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("hotkey manager: {0}")]
    Manager(String),
    #[error("register {label}: {detail}")]
    Register { label: &'static str, detail: String },
}

impl HotkeyManager {
    pub fn new() -> Result<Self, HotkeyError> {
        let manager =
            GlobalHotKeyManager::new().map_err(|err| HotkeyError::Manager(err.to_string()))?;
        let spotlight = HotKey::new(Some(Modifiers::ALT), Code::Space);
        let detail = HotKey::new(None, Code::F1);
        let chat = HotKey::new(None, Code::F2);
        let spotlight_id = spotlight.id();
        let detail_id = detail.id();
        let chat_id = chat.id();
        manager
            .register(spotlight)
            .map_err(|err| HotkeyError::Register {
                label: "Alt+Space",
                detail: err.to_string(),
            })?;
        manager.register(detail).map_err(|err| HotkeyError::Register {
            label: "F1",
            detail: err.to_string(),
        })?;
        manager.register(chat).map_err(|err| HotkeyError::Register {
            label: "F2",
            detail: err.to_string(),
        })?;
        Ok(Self {
            manager,
            spotlight_id,
            detail_id,
            chat_id,
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
        }
        None
    }
}
