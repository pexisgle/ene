//! Linux system tray backed by [ksni](https://crates.io/crates/ksni) (D-Bus SNI).

#![deny(unsafe_code)]

mod icon;
mod tray;

pub use tray::{LinuxTrayConfig, LinuxTrayError, LinuxTrayEvent, LinuxTrayHandle, TrayMenuSlot};
