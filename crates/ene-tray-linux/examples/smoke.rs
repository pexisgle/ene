//! Spawns a tray icon briefly to verify ksni registration under a D-Bus session.
//!
//! Run: `dbus-run-session -- cargo run -p ene-tray-linux --example smoke`

#![expect(
    clippy::expect_used,
    clippy::print_stderr,
    reason = "manual smoke example: panics on setup failure and prints success to stderr by design"
)]

use ene_tray_linux::{LinuxTrayConfig, LinuxTrayHandle, TrayMenuSlot};
use tokio::runtime::Runtime;

fn main() {
    let rt = Runtime::new().expect("tokio runtime");
    let icon = vec![0x40, 0x80, 0xFF, 0xFF];
    let config = LinuxTrayConfig {
        app_id: "ene-tray-smoke".into(),
        tooltip: "ene-tray-linux smoke".into(),
        icon_rgba: (icon, 1, 1),
        menu: vec![
            TrayMenuSlot::Item {
                id: "quit".into(),
                label: "Quit".into(),
                enabled: true,
            },
            TrayMenuSlot::Separator,
        ],
    };

    let handle = LinuxTrayHandle::spawn(config, rt.handle()).expect("spawn tray");
    std::thread::sleep(std::time::Duration::from_secs(2));
    drop(handle);
    eprintln!("smoke: tray spawned and dropped OK");
}
