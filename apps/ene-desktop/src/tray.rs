use bevy::app::AppExit;
use bevy::prelude::*;
use std::{fs::File, io::BufReader};
use tray_icon::{
    Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::app_config::CharacterSettings;

pub struct TrayPlugin;

const SETTINGS_MENU_ID: &str = "ene.settings";
const QUIT_MENU_ID: &str = "ene.quit";

impl Plugin for TrayPlugin {
    fn build(&self, app: &mut App) {
        setup_tray_icon(app);

        app.add_systems(Update, poll_tray_events);

        #[cfg(target_os = "linux")]
        app.add_systems(Update, tick_gtk_events);
    }
}

#[cfg(not(target_os = "windows"))]
struct TrayApp {
    _tray_icon: TrayIcon,
}

fn build_tray_menu() -> Menu {
    let menu = Menu::new();
    let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, "Settings", true, None);
    let quit_item = MenuItem::with_id(QUIT_MENU_ID, "Quit", true, None);
    let _ = menu.append_items(&[&settings_item, &PredefinedMenuItem::separator(), &quit_item]);
    menu
}

fn build_tray_icon_and_menu() -> TrayIcon {
    let menu = build_tray_menu();
    let icon = build_tray_icon().unwrap_or_else(|| {
        // Fallback to a dummy blue icon if loading PNG fails
        let (width, height) = (32, 32);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&[0, 128, 255, 255]);
        }
        Icon::from_rgba(rgba, width, height).unwrap()
    });

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ene")
        .with_icon(icon)
        .build()
        .unwrap()
}

/// Windows needs its own message pump thread; the Bevy `App` is not needed here.
#[cfg(target_os = "windows")]
fn setup_tray_icon(_app: &mut App) {
    std::thread::spawn(move || {
        let _tray_icon = build_tray_icon_and_menu();

        unsafe {
            let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
            while windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(
                &mut msg,
                std::ptr::null_mut(),
                0,
                0,
            ) > 0
            {
                windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            }
        }

        std::mem::forget(_tray_icon);
    });
}

/// Non-Windows platforms store the tray icon as a non-send resource so the
/// event loop can keep it alive.
#[cfg(not(target_os = "windows"))]
fn setup_tray_icon(app: &mut App) {
    app.insert_non_send_resource(TrayApp {
        _tray_icon: build_tray_icon_and_menu(),
    });
}

#[cfg(target_os = "linux")]
fn tick_gtk_events(_tray: NonSend<TrayApp>) {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

fn poll_tray_events(mut settings: ResMut<CharacterSettings>, mut app_exit: MessageWriter<AppExit>) {
    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            ..
        } = event
        {
            settings.ui.settings_window_visible = true;
        }
    }

    while let Ok(event) = MenuEvent::receiver().try_recv() {
        match event.id.as_ref() {
            SETTINGS_MENU_ID => {
                settings.ui.settings_window_visible = true;
            }
            QUIT_MENU_ID => {
                app_exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}

fn build_tray_icon() -> Option<Icon> {
    let path = ene_config::assets_dir().join("icon.png");
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) => {
            error!("Failed to open tray icon file {}: {err}", path.display());
            return None;
        }
    };

    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = match decoder.read_info() {
        Ok(reader) => reader,
        Err(err) => {
            error!(
                "Failed to read tray icon metadata {}: {err}",
                path.display()
            );
            return None;
        }
    };

    let Some(output_size) = reader.output_buffer_size() else {
        error!(
            "Failed to determine tray icon decode buffer size for {}",
            path.display()
        );
        return None;
    };
    let mut bytes = vec![0; output_size];
    let frame = match reader.next_frame(&mut bytes) {
        Ok(frame) => frame,
        Err(err) => {
            error!("Failed to decode tray icon PNG {}: {err}", path.display());
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
            for &v in src {
                out.extend_from_slice(&[v, v, v, 255]);
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
            error!(
                "Unsupported indexed PNG output for tray icon {}",
                path.display()
            );
            return None;
        }
    };

    match Icon::from_rgba(rgba, frame.width, frame.height) {
        Ok(icon) => Some(icon),
        Err(err) => {
            error!("Failed to create tray icon from {}: {err}", path.display());
            None
        }
    }
}
