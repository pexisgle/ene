//! First-party desktop GUI (`apps/ene-desktop`).
//!
//! Host is durable authority. This process talks the Client channel
//! ([`ene_client`]) and Host-local control ([`ene_local_control`]). It does
//! not know the DB schema, does not spawn Body as a requirement for chat or
//! settings, and holds no confirmation authority unless the Host itself
//! spawned this process and handed it the private channel.
//!
//! Modules match [First-party desktop](../../docs/design/concrete/first-party-desktop.md)
//! §6: `ui`, `session`, `control`, `body_supervise`, `i18n`, `erasure`.
//! `measure` owns slice F raw records and gate evaluation. A default or
//! incomplete record never claims a pass.

pub mod body_supervise;
pub mod control;
pub mod erasure;
pub mod i18n;
pub mod measure;
pub mod session;
pub mod ui;

pub mod host_launch;
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
