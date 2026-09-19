//! First-party desktop GUI (`apps/ene-desktop`).
//!
//! Host is durable authority. This process talks the Client channel
//! ([`ene_client`]) and, when seated, Host-local control
//! ([`ene_local_control`]). It does not know the DB schema, does not spawn
//! Body as a requirement for chat or settings, and does not treat empty-seat
//! occupancy as authenticity evidence.
//!
//! Modules match [First-party desktop](../../docs/design/concrete/first-party-desktop.md)
//! §6: `ui`, `session`, `control`, `body_supervise`, `i18n`, `erasure`.
//! `measure` is the slice F recording format only; it does not claim a gate.

pub mod body_supervise;
pub mod control;
pub mod erasure;
pub mod i18n;
pub mod measure;
pub mod session;
pub mod snapshot_pump;
pub mod ui;

pub(crate) mod host_launch;
pub(crate) mod secret;

pub use i18n::Locale;
pub use ui::{
    DesktopRuntime, GuiSnapshot, MemoryPage, MemoryRevisionRow, MemoryRow, Page, WizardStep,
};

/// Install-asset path for the bundled character `ene`.
///
/// The official VRM is not in this repository (GitHub issue #1651). Body
/// spawn is optional; a missing file is `BodyStatus::Absent`, never a
/// substitute character and never Alicia.
pub const BUNDLED_ENE_ASSET: &str = "assets/ene.vrm";

/// Device descriptor this GUI presents at pairing. Display only.
pub const DESKTOP_DESCRIPTOR: &str = "ene-desktop";
