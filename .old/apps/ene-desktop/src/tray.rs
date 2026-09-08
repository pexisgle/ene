//! System tray integration.
//!
//! On Windows, `tray-icon` runs a dedicated Win32 message-pump thread.
//! On Linux, [`ene_tray_linux`] serves the tray via D-Bus SNI (no GTK).
use std::fs::File;
use std::io::BufReader;

use crate::events::{AppEvent, AppEventSender, TrayAction};

const SETTINGS_MENU_ID: &str = "ene.settings";
const CHAT_MENU_ID: &str = "ene.chat";
const DETAIL_MENU_ID: &str = "ene.detail";
const QUIT_MENU_ID: &str = "ene.quit";
const TOOLTIP: &str = "ene";

pub struct TrayHandle {
    _private: (),
}

impl TrayHandle {
    /// Returns `None` if the icon cannot be constructed (e.g. on a
    /// headless build); the runtime should treat that as a soft failure.
    pub fn new(event_tx: AppEventSender, runtime: &tokio::runtime::Handle) -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            let icon = build_icon_rgba().unwrap_or_else(synthetic_icon_rgba);
            let menu = vec![
                ene_tray_linux::TrayMenuSlot::Item {
                    id: SETTINGS_MENU_ID.into(),
                    label: i18n_embed_fl::fl!(crate::i18n::loader(), "settings"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Item {
                    id: CHAT_MENU_ID.into(),
                    label: i18n_embed_fl::fl!(crate::i18n::loader(), "tray-chat"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Item {
                    id: DETAIL_MENU_ID.into(),
                    label: i18n_embed_fl::fl!(crate::i18n::loader(), "tray-detail"),
                    enabled: true,
                },
                ene_tray_linux::TrayMenuSlot::Separator,
                ene_tray_linux::TrayMenuSlot::Item {
                    id: QUIT_MENU_ID.into(),
                    label: i18n_embed_fl::fl!(crate::i18n::loader(), "quit"),
                    enabled: true,
                },
            ];
            let backend = ene_tray_linux::LinuxTrayHandle::spawn(
                ene_tray_linux::LinuxTrayConfig {
                    app_id: "ene-desktop".into(),
                    tooltip: TOOLTIP.into(),
                    icon_rgba: icon,
                    menu,
                },
                runtime,
            )
            .ok()?;
            std::thread::spawn(move || pump_linux_tray_events(backend, event_tx));
            // The pump thread owns the backend handle.
            Some(Self { _private: () })
        }

        #[cfg(target_os = "windows")]
        {
            let _ = runtime;
            install_windows_tray(event_tx);
            Some(Self { _private: () })
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = (event_tx, runtime);
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn pump_linux_tray_events(backend: ene_tray_linux::LinuxTrayHandle, event_tx: AppEventSender) {
    loop {
        while let Some(event) = backend.try_recv() {
            if let Some(action) = map_linux_event(event)
                && event_tx.send(AppEvent::Tray(action)).is_err()
            {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(target_os = "linux")]
fn map_linux_event(event: ene_tray_linux::LinuxTrayEvent) -> Option<TrayAction> {
    match event {
        ene_tray_linux::LinuxTrayEvent::MenuActivate { id } => match id.as_str() {
            SETTINGS_MENU_ID => Some(TrayAction::OpenSettings { page: None }),
            CHAT_MENU_ID => Some(TrayAction::OpenChat),
            DETAIL_MENU_ID => Some(TrayAction::OpenDetail),
            QUIT_MENU_ID => Some(TrayAction::Quit),
            _ => None,
        },
        ene_tray_linux::LinuxTrayEvent::IconActivate => {
            Some(TrayAction::OpenSettings { page: None })
        }
        ene_tray_linux::LinuxTrayEvent::IconDoubleActivate => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn build_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let path = ene_config::assets_dir().join("icon.png");
    let file = File::open(&path).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let output_size = reader.output_buffer_size()?;
    let mut bytes = vec![0u8; output_size];
    let frame = reader.next_frame(&mut bytes).ok()?;
    let src = &bytes[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((frame.width * frame.height * 4) as usize);
            for chunk in src.as_chunks::<3>().0 {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((frame.width * frame.height * 4) as usize);
            for v in src {
                out.extend_from_slice(&[*v, *v, *v, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((frame.width * frame.height * 4) as usize);
            for chunk in src.as_chunks::<2>().0 {
                out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            out
        }
        png::ColorType::Indexed => return None,
    };
    Some((rgba, frame.width, frame.height))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn synthetic_icon_rgba() -> (Vec<u8>, u32, u32) {
    let (w, h) = (32, 32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[0, 128, 255, 255]);
    }
    (rgba, w, h)
}

#[cfg(target_os = "windows")]
mod windows {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};

    use super::{
        CHAT_MENU_ID, DETAIL_MENU_ID, QUIT_MENU_ID, SETTINGS_MENU_ID, TOOLTIP, TrayAction,
        build_icon_rgba, synthetic_icon_rgba,
    };
    use crate::events::{AppEvent, AppEventSender};

    pub(super) fn install_windows_tray(event_tx: AppEventSender) {
        std::thread::spawn(move || {
            #[expect(
                clippy::expect_used,
                reason = "tray icon builder must succeed on Windows"
            )]
            let _tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(build_menu()))
                .with_tooltip(TOOLTIP)
                .with_icon(build_icon().unwrap_or_else(synthetic_icon))
                .build()
                .expect("tray icon must build on Windows");
            pump_win32_messages();
            #[expect(
                clippy::mem_forget,
                reason = "keeps the tray icon HWND alive for the life of the message-pump thread"
            )]
            std::mem::forget(_tray_icon);
        });
        std::thread::spawn(move || pump_tray_events(&event_tx));
    }

    fn build_menu() -> Menu {
        let menu = Menu::new();
        let settings_label = i18n_embed_fl::fl!(crate::i18n::loader(), "settings");
        let chat_label = i18n_embed_fl::fl!(crate::i18n::loader(), "tray-chat");
        let detail_label = i18n_embed_fl::fl!(crate::i18n::loader(), "tray-detail");
        let quit_label = i18n_embed_fl::fl!(crate::i18n::loader(), "quit");
        let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, settings_label, true, None);
        let chat_item = MenuItem::with_id(CHAT_MENU_ID, chat_label, true, None);
        let detail_item = MenuItem::with_id(DETAIL_MENU_ID, detail_label, true, None);
        let quit_item = MenuItem::with_id(QUIT_MENU_ID, quit_label, true, None);
        drop(menu.append_items(&[
            &settings_item,
            &chat_item,
            &detail_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ]));
        menu
    }

    fn build_icon() -> Option<Icon> {
        let (rgba, width, height) = build_icon_rgba()?;
        Icon::from_rgba(rgba, width, height).ok()
    }

    fn synthetic_icon() -> Icon {
        let (rgba, w, h) = synthetic_icon_rgba();
        #[expect(
            clippy::expect_used,
            reason = "synthetic RGBA is valid by construction"
        )]
        Icon::from_rgba(rgba, w, h).expect("tray-icon internal bug")
    }

    fn pump_win32_messages() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, TranslateMessage,
        };
        // SAFETY: `msg` is a valid, zero-initialized `MSG` on the stack
        // that outlives every call, and `GetMessageW` / `TranslateMessage` /
        // `DispatchMessageW` are invoked with `hWnd = null_mut()`, which
        // retrieves / dispatches messages for the current thread only.
        unsafe {
            let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn pump_tray_events(event_tx: &AppEventSender) {
        let tray_rx = TrayIconEvent::receiver().clone();
        let menu_rx = MenuEvent::receiver().clone();
        loop {
            crossbeam_channel::select! {
                recv(tray_rx) -> event => {
                    let Ok(event) = event else { break };
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        drop(event_tx.send(AppEvent::Tray(TrayAction::OpenSettings { page: None })));
                    }
                }
                recv(menu_rx) -> event => {
                    let Ok(event) = event else { break };
                    let action = match event.id.as_ref() {
                        SETTINGS_MENU_ID => Some(TrayAction::OpenSettings { page: None }),
                        CHAT_MENU_ID => Some(TrayAction::OpenChat),
                        DETAIL_MENU_ID => Some(TrayAction::OpenDetail),
                        QUIT_MENU_ID => Some(TrayAction::Quit),
                        _ => None,
                    };
                    if let Some(action) = action {
                        drop(event_tx.send(AppEvent::Tray(action)));
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
use windows::install_windows_tray;
