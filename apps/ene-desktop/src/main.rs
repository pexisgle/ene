//! First-party desktop binary: launcher plus Slint window.
//!
//! Host is detached if it is not already serving. Closing this process does
//! not stop Host. Body is optional.

use std::path::PathBuf;

use ene_config::{Config, resolve_data_dir};
use ene_desktop::ui::{DesktopError, DesktopRuntime};
use ene_desktop_ui::AppWindow;
use slint::{ComponentHandle as _, SharedString};

fn main() -> Result<(), DesktopError> {
    let config = Config::load(None).map_err(|error| DesktopError::Protocol(error.to_string()))?;
    let data_dir: PathBuf = resolve_data_dir(&config)
        .ok_or_else(|| DesktopError::HostLaunch(String::from("no data directory resolved")))?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| DesktopError::Transport(error.to_string()))?;
    let mut desktop = DesktopRuntime::new(data_dir);
    match desktop.ensure_host(None) {
        Ok(()) | Err(_) => {}
    }
    desktop.try_spawn_body(&desktop.bundled_ene_asset());
    let window = AppWindow::new().map_err(|error| DesktopError::Protocol(error.to_string()))?;
    apply_snapshot(&window, &desktop);
    window.on_open_chat({
        let ui = window.as_weak();
        move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_page(ene_desktop_ui::UiPage::Chat);
            }
        }
    });
    window.on_open_history({
        let ui = window.as_weak();
        move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_page(ene_desktop_ui::UiPage::History);
            }
        }
    });
    window.on_open_settings({
        let ui = window.as_weak();
        move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_page(ene_desktop_ui::UiPage::Settings);
            }
        }
    });
    window.on_open_about({
        let ui = window.as_weak();
        move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_page(ene_desktop_ui::UiPage::About);
            }
        }
    });
    let _guard = tokio_runtime.enter();
    window
        .run()
        .map_err(|error| DesktopError::Protocol(error.to_string()))
}

fn apply_snapshot(window: &AppWindow, desktop: &DesktopRuntime) {
    let snap = desktop.snapshot();
    window.set_locale(SharedString::from(snap.locale.as_str()));
    window.set_heading(SharedString::from("ene"));
    window.set_status(SharedString::from(snap.connection.as_str()));
    let timeline = snap.timeline.join("\n");
    window.set_timeline(SharedString::from(timeline.as_str()));
    let history = snap.history.join("\n");
    window.set_history(SharedString::from(history.as_str()));
    window.set_about_title(SharedString::from("ene"));
}
