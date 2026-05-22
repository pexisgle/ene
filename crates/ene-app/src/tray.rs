use bevy::app::AppExit;
use bevy::prelude::*;
#[cfg(target_os = "linux")]
use std::thread;
use std::{fs::File, io::BufReader};
#[cfg(not(target_os = "linux"))]
use tray_icon::TrayIcon;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
};

use crate::app_config::CharacterSettings;

pub struct TrayPlugin;

const SETTINGS_MENU_ID: &str = "ene.settings";
const QUIT_MENU_ID: &str = "ene.quit";
const TRAY_ICON_ID: &str = "ene.tray";

impl Plugin for TrayPlugin {
    fn build(&self, app: &mut App) {
        app.insert_non_send_resource(TrayState::default())
            .add_systems(Startup, setup_tray_icon)
            .add_systems(Update, poll_tray_events);
    }
}

#[derive(Default)]
struct TrayState {
    #[cfg(not(target_os = "linux"))]
    tray: Option<TrayIcon>,
    settings_item_id: Option<MenuId>,
    quit_item_id: Option<MenuId>,
}

#[cfg(target_os = "linux")]
fn ensure_gtk_initialized() -> bool {
    if gtk::is_initialized_main_thread() {
        return true;
    }

    match gtk::init() {
        Ok(()) => true,
        Err(err) => {
            error!("Failed to initialize GTK for tray icon: {err}");
            false
        }
    }
}

fn setup_tray_icon(mut tray_state: NonSendMut<TrayState>) {
    let tray_state = &mut *tray_state;

    if tray_state
        .settings_item_id
        .as_ref()
        .is_some_and(|id| id == SETTINGS_MENU_ID)
    {
        return;
    }

    tray_state.settings_item_id = Some(MenuId::new(SETTINGS_MENU_ID));
    tray_state.quit_item_id = Some(MenuId::new(QUIT_MENU_ID));

    #[cfg(target_os = "linux")]
    {
        if thread::Builder::new()
            .name("ene-tray-gtk".to_string())
            .spawn(|| {
                if !ensure_gtk_initialized() {
                    return;
                }

                let Some(icon) = build_tray_icon() else {
                    error!("Failed to build tray icon image");
                    return;
                };

                let menu = Menu::new();
                let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, "Settings", true, None);
                let quit_item = MenuItem::with_id(QUIT_MENU_ID, "Quit", true, None);

                if menu
                    .append_items(&[&settings_item, &PredefinedMenuItem::separator(), &quit_item])
                    .is_err()
                {
                    error!("Failed to build tray menu");
                    return;
                }

                let tray = match TrayIconBuilder::new()
                    .with_id(TRAY_ICON_ID)
                    .with_icon(icon)
                    .with_tooltip("Ene")
                    .with_title("Ene")
                    .with_menu(Box::new(menu))
                    .with_menu_on_left_click(false)
                    .build()
                {
                    Ok(tray) => tray,
                    Err(err) => {
                        error!("Failed to create tray icon: {err}");
                        return;
                    }
                };

                // Keep tray alive while the GTK loop dispatches StatusNotifier events.
                let _tray = tray;
                gtk::main();
            })
            .is_err()
        {
            error!("Failed to spawn GTK tray thread");
        }
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        if tray_state.tray.is_some() {
            return;
        }

        let Some(icon) = build_tray_icon() else {
            error!("Failed to build tray icon image");
            return;
        };

        let menu = Menu::new();
        let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, "Settings", true, None);
        let quit_item = MenuItem::with_id(QUIT_MENU_ID, "Quit", true, None);
        let settings_item_id = settings_item.id().clone();
        let quit_item_id = quit_item.id().clone();

        if menu
            .append_items(&[&settings_item, &PredefinedMenuItem::separator(), &quit_item])
            .is_err()
        {
            error!("Failed to build tray menu");
            return;
        }

        match TrayIconBuilder::new()
            .with_id(TRAY_ICON_ID)
            .with_icon(icon)
            .with_tooltip("Ene")
            .with_title("Ene")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
        {
            Ok(tray) => {
                tray_state.tray = Some(tray);
                tray_state.settings_item_id = Some(settings_item_id);
                tray_state.quit_item_id = Some(quit_item_id);
            }
            Err(err) => {
                error!("Failed to create tray icon: {err}");
            }
        }
    }
}

fn poll_tray_events(
    mut settings: ResMut<CharacterSettings>,
    mut app_exit: MessageWriter<AppExit>,
    tray_state: NonSend<TrayState>,
) {
    let tray_state = &*tray_state;

    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        if let TrayIconEvent::Click {
            button,
            button_state,
            ..
        } = event
            && button == MouseButton::Left
            && button_state == MouseButtonState::Up
        {
            settings.ui.settings_window_visible = true;
        }
    }

    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if tray_state
            .settings_item_id
            .as_ref()
            .is_some_and(|id| id == &event.id)
        {
            settings.ui.settings_window_visible = true;
            continue;
        }

        if tray_state
            .quit_item_id
            .as_ref()
            .is_some_and(|id| id == &event.id)
        {
            app_exit.write(AppExit::Success);
        }
    }
}

fn build_tray_icon() -> Option<Icon> {
    let path = ene_ai_core::paths::assets_dir().join("icon.png");
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
