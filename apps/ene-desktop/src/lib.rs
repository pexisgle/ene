pub mod body_supervise;
pub mod control;
pub mod erasure;
pub mod i18n;
pub mod measure;
pub mod session;
pub mod ui;

pub mod host_launch;
pub mod motion;
pub(crate) mod secret;

pub use motion::BUNDLED_MOTION_DIR;

pub const BUNDLED_SAMPLE_ASSET: &str = "assets/seed-san.vrm";

pub const DESKTOP_DESCRIPTOR: &str = "ene-desktop";
