//! Desktop shell integration: tray, global hotkeys, notifications.

pub mod hotkeys;
pub mod notify;
pub mod tray;

use thiserror::Error;

pub use hotkeys::{HotkeyManager, ShellAction};
pub use notify::show_notification;
pub use tray::{TrayAction, TrayManager};

/// Initialize tracing for the stage binary.
pub fn init_tracing() {
    if tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .is_err()
    {
        // Subscriber already registered in this process.
    }
}

/// Errors from shell subsystems.
#[derive(Debug, Error)]
pub enum ShellError {
    #[error("tray: {0}")]
    Tray(#[from] tray::TrayError),
    #[error("hotkeys: {0}")]
    Hotkeys(#[from] hotkeys::HotkeyError),
    #[error("notify: {0}")]
    Notify(#[from] notify::NotifyError),
}
