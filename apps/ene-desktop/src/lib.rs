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

pub use i18n::Locale;
pub use motion::BUNDLED_MOTION_DIR;
pub use ui::{
    DesktopRuntime, GuiSnapshot, MemoryPage, MemoryRevisionRow, MemoryRow, Page, WizardStep,
};

pub const BUNDLED_SAMPLE_ASSET: &str = "assets/seed-san.vrm";

pub const DESKTOP_DESCRIPTOR: &str = "ene-desktop";
