//! System tray integration.
//!
//! The tray icon and its menu live in a separate OS-managed
//! resource. On Windows, `tray-icon` (with the default winapi
//! backend) requires its own Win32 message-pump thread. On Linux,
//! `tray-icon` (with the `gtk` feature) requires GTK to be ticked
//! on the main thread.
//!
//! This module abstracts that away behind [`TrayHandle`]:
//!
//! - On Windows: `TrayHandle::new` spawns a thread that owns the
//!   tray icon and pumps `GetMessageW`. The thread sends
//!   [`TrayAction`]s into the cross-subsystem bus and forgets the
//!   icon so it is not dropped when the thread returns.
//! - On Linux: `TrayHandle::new` constructs the tray icon on the
//!   calling (main) thread. The runtime must call
//!   [`TrayHandle::tick_gtk`] from `about_to_wait` to pump
//!   `gtk::main_iteration_do(false)` while events are pending.
//!
//! In both cases, `MenuEvent`s and `TrayIconEvent::Click`s are
//! translated into [`AppEvent::Tray`].
use std::fs::File;
use std::io::BufReader;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, TrayIconBuilder, TrayIconEvent};

use crate::events::{AppEvent, AppEventSender, TrayAction};

const SETTINGS_MENU_ID: &str = "ene.settings";
const QUIT_MENU_ID: &str = "ene.quit";
const TOOLTIP: &str = "ene";

/// Owns the tray icon (and, on Windows, the pump thread).
pub struct TrayHandle {
    /// Drop guard for the tray icon on Linux. On Windows the icon
    /// lives in the dedicated thread and is intentionally
    /// `mem::forgotten`, so this field is unused.
    #[cfg(target_os = "linux")]
    _icon: TrayIcon,
}

impl TrayHandle {
    /// Build a new tray icon and wire it to the given event
    /// sender. Returns `None` if the icon cannot be constructed
    /// (e.g. on a headless build); the runtime should treat that as
    /// a soft failure.
    pub fn new(event_tx: AppEventSender) -> Option<Self> {
        // Forward click + menu events from the (potentially
        // different-thread) tray-icon global receivers into the bus.
        // The channel is unbounded so `send` is non-blocking.
        //
        // IMPORTANT: on Windows the icon must be built **on the same
        // thread** that runs the Win32 message pump — building it on
        // the calling (main) thread and then calling
        // `mem::forget` creates a second, orphaned icon in the
        // notification area. So the Windows path builds the icon
        // inside the spawned thread; the Linux path builds it on the
        // main thread because the GTK backend requires it there.
        install_event_pump(event_tx);

        #[cfg(target_os = "linux")]
        {
            let menu = build_menu();
            let icon = build_icon().unwrap_or_else(synthetic_icon);

            let tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(TOOLTIP)
                .with_icon(icon)
                .build()
                .ok()?;

            Some(Self { _icon: tray_icon })
        }

        #[cfg(target_os = "windows")]
        {
            // Icon lives in the spawned thread (see
            // `install_event_pump`); nothing to keep here.
            Some(Self {})
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = event_tx;
            None
        }
    }

    /// Pump pending GTK events. No-op on Windows.
    #[cfg(target_os = "linux")]
    pub fn tick_gtk(&self) {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

fn build_menu() -> Menu {
    let menu = Menu::new();
    let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, "Settings", true, None);
    let quit_item = MenuItem::with_id(QUIT_MENU_ID, "Quit", true, None);
    let _ = menu.append_items(&[&settings_item, &PredefinedMenuItem::separator(), &quit_item]);
    menu
}

fn build_icon() -> Option<Icon> {
    let path = ene_config::assets_dir().join("icon.png");
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!(
                "Failed to open tray icon {}: {err}; using synthetic fallback",
                path.display()
            );
            return None;
        }
    };

    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = match decoder.read_info() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!("Failed to read tray icon metadata: {err}");
            return None;
        }
    };
    let Some(output_size) = reader.output_buffer_size() else {
        tracing::warn!("Failed to determine tray icon decode buffer size");
        return None;
    };
    let mut bytes = vec![0u8; output_size];
    let frame = match reader.next_frame(&mut bytes) {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!("Failed to decode tray icon PNG: {err}");
            return None;
        }
    };
    let src = &bytes[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => src.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((frame.width * frame.height * 4) as usize);
            for chunk in src.chunks_exact(3) {
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
            for chunk in src.chunks_exact(2) {
                out.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            out
        }
        png::ColorType::Indexed => {
            tracing::warn!("Unsupported indexed PNG color type for tray icon");
            return None;
        }
    };
    match Icon::from_rgba(rgba, frame.width, frame.height) {
        Ok(icon) => Some(icon),
        Err(err) => {
            tracing::warn!("Failed to build tray icon from RGBA: {err}");
            None
        }
    }
}

fn synthetic_icon() -> Icon {
    let (w, h) = (32, 32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[0, 128, 255, 255]);
    }
    Icon::from_rgba(rgba, w, h).expect("32x32 RGBA must be a valid tray icon")
}

/// Spawn the per-platform event pump that translates
/// `tray_icon`'s global event receivers into [`AppEvent::Tray`].
fn install_event_pump(event_tx: AppEventSender) {
    #[cfg(target_os = "windows")]
    {
        std::thread::spawn(move || {
            // Build + own the icon on this thread; the
            // `tray-icon` Win32 backend needs a thread-local
            // message pump. Forgetting the icon at the end of this
            // closure keeps the notification-area entry alive for
            // the lifetime of the process.
            let _tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(build_menu()))
                .with_tooltip(TOOLTIP)
                .with_icon(build_icon().unwrap_or_else(synthetic_icon))
                .build()
                .expect("tray icon must build on Windows");
            // The Win32 message pump is what dispatches WM_RBUTTONUP
            // to the tray icon's window proc, which in turn shows
            // the context menu attached via `with_menu(...)`.
            // Without this loop, right-click on the tray icon does
            // nothing — the OS sees the click but nobody is
            // processing the message.
            pump_win32_messages();
            pump_tray_events(&event_tx);
            std::mem::forget(_tray_icon);
        });
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux the icon is owned by the runtime (in
        // `TrayHandle::_icon`). The pump just polls the global
        // receivers — `tray-icon` is single-threaded but the
        // receivers are `Send`.
        std::thread::spawn(move || {
            pump_tray_events(&event_tx);
        });
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = event_tx;
    }
}

#[cfg(target_os = "windows")]
fn pump_win32_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage,
    };
    unsafe {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn pump_tray_events(event_tx: &AppEventSender) {
    loop {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                let _ = event_tx.send(AppEvent::Tray(TrayAction::OpenSettings));
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let action = match event.id.as_ref() {
                SETTINGS_MENU_ID => Some(TrayAction::OpenSettings),
                QUIT_MENU_ID => Some(TrayAction::Quit),
                _ => None,
            };
            if let Some(action) = action {
                let _ = event_tx.send(AppEvent::Tray(action));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
