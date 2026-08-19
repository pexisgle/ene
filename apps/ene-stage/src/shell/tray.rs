//! System tray menu for `ene-stage`.
//!
//! # Linux / GTK
//!
//! On Linux, GTK must be initialized in `main` before constructing a
//! [`TrayManager`], for example:
//!
//! ```ignore
//! #[cfg(target_os = "linux")]
//! {
//!     gtk::init().expect("gtk init");
//! }
//! ```

use crossbeam_channel::{Receiver, Sender, unbounded};
use thiserror::Error;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

const DETAIL_ID: &str = "ene-stage.tray.detail";
const CHAT_ID: &str = "ene-stage.tray.chat";
const MIC_ID: &str = "ene-stage.tray.mic";
const QUIT_ID: &str = "ene-stage.tray.quit";

/// Actions emitted from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenDetail,
    OpenChatFocus,
    ToggleMic,
    Quit,
}

/// Tray icon + menu. Poll [`TrayManager::try_recv`] from the UI loop.
pub struct TrayManager {
    _icon: TrayIcon,
    action_rx: Receiver<TrayAction>,
}

#[derive(Debug, Error)]
pub enum TrayError {
    #[error("tray icon: {0}")]
    Build(String),
    #[error("icon decode: {0}")]
    Icon(String),
}

impl TrayManager {
    /// Build the tray icon and wire menu events into an internal channel.
    pub fn new() -> Result<Self, TrayError> {
        let (action_tx, action_rx) = unbounded();
        let icon = build_icon()?;
        let menu = build_menu()?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ene-stage")
            .with_icon(icon)
            .build()
            .map_err(|err| TrayError::Build(err.to_string()))?;

        std::thread::spawn(move || poll_tray_events(&action_tx));

        Ok(Self {
            _icon: tray,
            action_rx,
        })
    }

    pub fn try_recv(&self) -> Option<TrayAction> {
        self.action_rx.try_recv().ok()
    }
}

fn build_menu() -> Result<Menu, TrayError> {
    let detail = MenuItem::with_id(MenuId::new(DETAIL_ID), "Open Detail", true, None);
    let chat = MenuItem::with_id(MenuId::new(CHAT_ID), "Open Chat", true, None);
    let mic = MenuItem::with_id(MenuId::new(MIC_ID), "Toggle Mic", true, None);
    let quit = MenuItem::with_id(MenuId::new(QUIT_ID), "Quit", true, None);
    Menu::with_items(&[
        &detail,
        &chat,
        &PredefinedMenuItem::separator(),
        &mic,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|err| TrayError::Build(err.to_string()))
}

fn build_icon() -> Result<Icon, TrayError> {
    let size = 16u32;
    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = 120;
            rgba[idx + 1] = 80;
            rgba[idx + 2] = 200;
            rgba[idx + 3] = 255;
        }
    }
    Icon::from_rgba(rgba, size, size).map_err(|err| TrayError::Icon(err.to_string()))
}

fn poll_tray_events(action_tx: &Sender<TrayAction>) {
    loop {
        if let Ok(event) = MenuEvent::receiver().try_recv()
            && let Some(action) = map_menu_id(&event.id)
            && action_tx.send(action).is_err()
        {
            return;
        }
        if let Ok(event) = TrayIconEvent::receiver().try_recv()
            && matches!(event, TrayIconEvent::DoubleClick { .. })
            && action_tx.send(TrayAction::OpenDetail).is_err()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn map_menu_id(id: &MenuId) -> Option<TrayAction> {
    let raw = id.0.as_str();
    match raw {
        DETAIL_ID => Some(TrayAction::OpenDetail),
        CHAT_ID => Some(TrayAction::OpenChatFocus),
        MIC_ID => Some(TrayAction::ToggleMic),
        QUIT_ID => Some(TrayAction::Quit),
        _ => None,
    }
}
