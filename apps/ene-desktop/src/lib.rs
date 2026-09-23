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

/// Install-asset path for the bundled VRM 1.0 sample model.
///
/// The bundled asset is Seed-san, not the official `ene` character; its
/// attribution and license are recorded in `assets/README.md`. Body spawn is
/// optional; a missing file is `BodyStatus::Absent`, never a substitute
/// character.
pub const BUNDLED_SAMPLE_ASSET: &str = "assets/seed-san.vrm";

pub const DESKTOP_DESCRIPTOR: &str = "ene-desktop";
