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

use std::path::Path;

use crossbeam_channel::{Receiver, Sender, unbounded};
use thiserror::Error;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::ShellCommand;
use crate::detail::DetailTab;

const SETTINGS_ID: &str = "ene-stage.tray.settings";
const CHAT_ID: &str = "ene-stage.tray.chat";
const DETAIL_ID: &str = "ene-stage.tray.detail";
const MIC_ID: &str = "ene-stage.tray.mic";
const QUIT_ID: &str = "ene-stage.tray.quit";

/// Tray icon + menu. Poll [`TrayManager::try_recv`] from the UI loop.
pub struct TrayManager {
    _icon: TrayIcon,
    action_rx: Receiver<ShellCommand>,
    mic_item: MenuItem,
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
        let (menu, mic_item) = build_menu()?;
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
            mic_item,
        })
    }

    pub fn try_recv(&self) -> Option<ShellCommand> {
        self.action_rx.try_recv().ok()
    }

    pub fn set_mic_active(&self, active: bool) {
        self.mic_item
            .set_text(crate::i18n::fl(mic_menu_label(active)));
    }
}

fn build_menu() -> Result<(Menu, MenuItem), TrayError> {
    let settings = MenuItem::with_id(
        MenuId::new(SETTINGS_ID),
        crate::i18n::fl("tray-settings"),
        true,
        None,
    );
    let detail = MenuItem::with_id(
        MenuId::new(DETAIL_ID),
        crate::i18n::fl("tray-detail"),
        true,
        None,
    );
    let chat = MenuItem::with_id(
        MenuId::new(CHAT_ID),
        crate::i18n::fl("tray-chat"),
        true,
        None,
    );
    let mic = MenuItem::with_id(
        MenuId::new(MIC_ID),
        crate::i18n::fl(mic_menu_label(false)),
        true,
        None,
    );
    let quit = MenuItem::with_id(
        MenuId::new(QUIT_ID),
        crate::i18n::fl("tray-quit"),
        true,
        None,
    );
    let menu = Menu::with_items(&[
        &settings,
        &chat,
        &detail,
        &PredefinedMenuItem::separator(),
        &mic,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|err| TrayError::Build(err.to_string()))?;
    Ok((menu, mic))
}

fn mic_menu_label(active: bool) -> &'static str {
    if active { "mic-on" } else { "mic-off" }
}

fn build_icon() -> Result<Icon, TrayError> {
    let path = ene_config::assets_dir().join("icon.png");
    match load_icon(&path) {
        Ok(icon) => Ok(icon),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to load tray icon asset; using synthetic fallback"
            );
            synthetic_icon()
        }
    }
}

fn load_icon(path: &Path) -> Result<Icon, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|err| err.to_string())?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).map_err(|err| err.to_string())
}

fn synthetic_icon() -> Result<Icon, TrayError> {
    let (width, height) = (32_u32, 32_u32);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        rgba.extend_from_slice(&[0, 128, 255, 255]);
    }
    Icon::from_rgba(rgba, width, height).map_err(|err| TrayError::Icon(err.to_string()))
}

fn poll_tray_events(action_tx: &Sender<ShellCommand>) {
    loop {
        if let Ok(event) = MenuEvent::receiver().try_recv()
            && let Some(action) = map_menu_id(&event.id)
            && action_tx.send(action).is_err()
        {
            return;
        }
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            let open_detail = matches!(event, TrayIconEvent::DoubleClick { .. });
            if open_detail
                && action_tx
                    .send(ShellCommand::OpenDetail(DetailTab::Home))
                    .is_err()
            {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn map_menu_id(id: &MenuId) -> Option<ShellCommand> {
    let raw = id.0.as_str();
    match raw {
        SETTINGS_ID => Some(ShellCommand::OpenDetail(DetailTab::System)),
        CHAT_ID => Some(ShellCommand::OpenChat),
        DETAIL_ID => Some(ShellCommand::OpenDetail(DetailTab::Home)),
        MIC_ID => Some(ShellCommand::ToggleMic),
        QUIT_ID => Some(ShellCommand::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_shared_shell_commands() {
        assert_eq!(
            map_menu_id(&MenuId::new(SETTINGS_ID)),
            Some(ShellCommand::OpenDetail(DetailTab::System))
        );
        assert_eq!(
            map_menu_id(&MenuId::new(CHAT_ID)),
            Some(ShellCommand::OpenChat)
        );
        assert_eq!(
            map_menu_id(&MenuId::new(DETAIL_ID)),
            Some(ShellCommand::OpenDetail(DetailTab::Home))
        );
        assert_eq!(
            map_menu_id(&MenuId::new(MIC_ID)),
            Some(ShellCommand::ToggleMic)
        );
        assert_eq!(map_menu_id(&MenuId::new(QUIT_ID)), Some(ShellCommand::Quit));
        assert_eq!(map_menu_id(&MenuId::new("unknown")), None);
    }

    #[test]
    fn shared_icon_asset_decodes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon.png");
        assert!(load_icon(&path).is_ok());
    }

    #[test]
    fn synthetic_icon_is_valid() {
        assert!(synthetic_icon().is_ok());
    }

    #[test]
    fn mic_menu_label_is_the_next_action() {
        assert_eq!(mic_menu_label(false), "mic-off");
        assert_eq!(mic_menu_label(true), "mic-on");
    }
}
