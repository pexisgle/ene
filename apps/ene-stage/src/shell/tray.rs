//! System tray menu for `ene-stage`.

use std::path::Path;

use thiserror::Error;
use tokio::runtime::Handle;

use super::ShellCommand;
use crate::detail::DetailTab;

const SETTINGS_ID: &str = "ene-stage.tray.settings";
const CHAT_ID: &str = "ene-stage.tray.chat";
const DETAIL_ID: &str = "ene-stage.tray.detail";
const MIC_ID: &str = "ene-stage.tray.mic";
const QUIT_ID: &str = "ene-stage.tray.quit";

/// Tray icon + menu. Poll [`TrayManager::try_recv`] from the UI loop.
pub struct TrayManager {
    #[cfg(target_os = "linux")]
    backend: ene_tray_linux::LinuxTrayHandle,
    #[cfg(target_os = "windows")]
    backend: WindowsTrayBackend,
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
    pub fn new(runtime: &Handle) -> Result<Self, TrayError> {
        #[cfg(target_os = "linux")]
        {
            let icon = load_icon_rgba();
            let menu = vec![
                ene_tray_linux::TrayMenuSlot::Item {
                    id: SETTINGS_ID.into(),
                    label: crate::i18n::fl("tray-settings"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Item {
                    id: CHAT_ID.into(),
                    label: crate::i18n::fl("tray-chat"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Item {
                    id: DETAIL_ID.into(),
                    label: crate::i18n::fl("tray-detail"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Separator,
                ene_tray_linux::TrayMenuSlot::Item {
                    id: MIC_ID.into(),
                    label: crate::i18n::fl(mic_menu_label(false)),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Separator,
                ene_tray_linux::TrayMenuSlot::Item {
                    id: QUIT_ID.into(),
                    label: crate::i18n::fl("tray-quit"),
                    enabled: true,
                },
            ];
            let backend = ene_tray_linux::LinuxTrayHandle::spawn(
                ene_tray_linux::LinuxTrayConfig {
                    app_id: "ene-stage".into(),
                    tooltip: "ene-stage".into(),
                    icon_rgba: icon,
                    menu,
                },
                runtime,
            )
            .map_err(|err| TrayError::Build(err.to_string()))?;
            Ok(Self { backend })
        }

        #[cfg(target_os = "windows")]
        {
            let _ = runtime;
            Ok(Self {
                backend: WindowsTrayBackend::new()?,
            })
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = runtime;
            Err(TrayError::Build(
                "tray is unsupported on this platform".into(),
            ))
        }
    }

    pub fn try_recv(&self) -> Option<ShellCommand> {
        #[cfg(target_os = "linux")]
        {
            self.backend.try_recv().and_then(map_linux_event)
        }
        #[cfg(target_os = "windows")]
        {
            self.backend.try_recv()
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            None
        }
    }

    #[must_use]
    pub fn take_interactions(&self) -> usize {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            self.backend.take_interactions()
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            0
        }
    }

    pub fn set_mic_active(&self, active: bool) {
        #[cfg(target_os = "linux")]
        {
            self.backend
                .set_item_label(MIC_ID, crate::i18n::fl(mic_menu_label(active)));
        }
        #[cfg(target_os = "windows")]
        {
            self.backend.set_mic_active(active);
        }
    }
}

#[cfg(target_os = "linux")]
fn map_linux_event(event: ene_tray_linux::LinuxTrayEvent) -> Option<ShellCommand> {
    match event {
        ene_tray_linux::LinuxTrayEvent::MenuActivate { id } => map_menu_id(&id),
        ene_tray_linux::LinuxTrayEvent::IconDoubleActivate => {
            Some(ShellCommand::OpenDetail(DetailTab::Home))
        }
        ene_tray_linux::LinuxTrayEvent::IconActivate => None,
    }
}

fn map_menu_id(id: &str) -> Option<ShellCommand> {
    match id {
        SETTINGS_ID => Some(ShellCommand::OpenDetail(DetailTab::System)),
        CHAT_ID => Some(ShellCommand::OpenChat),
        DETAIL_ID => Some(ShellCommand::OpenDetail(DetailTab::Home)),
        MIC_ID => Some(ShellCommand::ToggleMic),
        QUIT_ID => Some(ShellCommand::Quit),
        _ => None,
    }
}

fn mic_menu_label(active: bool) -> &'static str {
    if active { "mic-on" } else { "mic-off" }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn load_icon_rgba() -> (Vec<u8>, u32, u32) {
    let path = ene_config::assets_dir().join("icon.png");
    load_icon_bytes(&path).unwrap_or_else(|err| {
        tracing::warn!(
            path = %path.display(),
            error = %err,
            "failed to load tray icon asset; using synthetic fallback"
        );
        synthetic_icon_rgba()
    })
}

fn load_icon_bytes(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|err| err.to_string())?;
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((rgba.into_raw(), width, height))
}

fn synthetic_icon_rgba() -> (Vec<u8>, u32, u32) {
    let (width, height) = (32_u32, 32_u32);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        rgba.extend_from_slice(&[0, 128, 255, 255]);
    }
    (rgba, width, height)
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::Path;

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{
        CHAT_ID, DETAIL_ID, DetailTab, MIC_ID, QUIT_ID, SETTINGS_ID, ShellCommand, TrayError,
        load_icon_bytes, map_menu_id, mic_menu_label, synthetic_icon_rgba,
    };

    pub(super) struct WindowsTrayBackend {
        _icon: TrayIcon,
        action_rx: Receiver<ShellCommand>,
        interaction_rx: Receiver<()>,
        mic_item: MenuItem,
    }

    impl WindowsTrayBackend {
        pub(super) fn new() -> Result<Self, TrayError> {
            let (action_tx, action_rx) = unbounded();
            let (interaction_tx, interaction_rx) = unbounded();
            let icon = build_icon()?;
            let (menu, mic_item) = build_menu()?;
            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("ene-stage")
                .with_icon(icon)
                .build()
                .map_err(|err| TrayError::Build(err.to_string()))?;

            std::thread::spawn(move || poll_tray_events(&action_tx, &interaction_tx));

            Ok(Self {
                _icon: tray,
                action_rx,
                interaction_rx,
                mic_item,
            })
        }

        pub(super) fn try_recv(&self) -> Option<ShellCommand> {
            self.action_rx.try_recv().ok()
        }

        pub(super) fn take_interactions(&self) -> usize {
            let mut drained = 0;
            while self.interaction_rx.try_recv().is_ok() {
                drained += 1;
            }
            drained
        }

        pub(super) fn set_mic_active(&self, active: bool) {
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

    fn build_icon() -> Result<Icon, TrayError> {
        let (rgba, width, height) = super::load_icon_rgba();
        Icon::from_rgba(rgba, width, height).map_err(|err| TrayError::Icon(err.to_string()))
    }

    fn poll_tray_events(action_tx: &Sender<ShellCommand>, interaction_tx: &Sender<()>) {
        loop {
            if let Ok(event) = MenuEvent::receiver().try_recv()
                && let Some(action) = map_menu_id(&event.id.0)
            {
                if interaction_tx.send(()).is_err() {
                    return;
                }
                if action_tx.send(action).is_err() {
                    return;
                }
            }
            if let Ok(event) = TrayIconEvent::receiver().try_recv() {
                let is_click = matches!(
                    event,
                    TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
                );
                let open_detail = matches!(event, TrayIconEvent::DoubleClick { .. });
                if is_click && interaction_tx.send(()).is_err() {
                    return;
                }
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
}

#[cfg(target_os = "windows")]
use windows::WindowsTrayBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_shared_shell_commands() {
        assert_eq!(
            map_menu_id(SETTINGS_ID),
            Some(ShellCommand::OpenDetail(DetailTab::System))
        );
        assert_eq!(map_menu_id(CHAT_ID), Some(ShellCommand::OpenChat));
        assert_eq!(
            map_menu_id(DETAIL_ID),
            Some(ShellCommand::OpenDetail(DetailTab::Home))
        );
        assert_eq!(map_menu_id(MIC_ID), Some(ShellCommand::ToggleMic));
        assert_eq!(map_menu_id(QUIT_ID), Some(ShellCommand::Quit));
        assert_eq!(map_menu_id("unknown"), None);
    }

    #[test]
    fn shared_icon_asset_decodes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon.png");
        assert!(load_icon_bytes(&path).is_ok());
    }

    #[test]
    fn synthetic_icon_is_valid() {
        let (rgba, width, height) = synthetic_icon_rgba();
        assert_eq!(rgba.len(), (width * height * 4) as usize);
    }

    #[test]
    fn mic_menu_label_is_the_next_action() {
        assert_eq!(mic_menu_label(false), "mic-off");
        assert_eq!(mic_menu_label(true), "mic-on");
    }
}
