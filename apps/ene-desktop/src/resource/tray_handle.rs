//! Tray icon glue.
//!
//! `tray_icon::TrayIcon` is not `Send + Sync`, so it cannot
//! be inserted as a bevy `Resource` (the `Resource` trait
//! requires `Component`, which requires `Send + Sync`); the icon
//! stays on `AppState::tray`. The Linux-only `tick_gtk` helper
//! below is invoked from
//! [`crate::runtime::Runtime::about_to_wait`] directly.

/// Linux-only: pump pending GTK events while the tray is
/// active.
#[cfg(target_os = "linux")]
pub fn tick_gtk() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

/// Non-Linux no-op.
#[cfg(not(target_os = "linux"))]
pub fn tick_gtk() {}
