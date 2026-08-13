//! Managed `__SIDECAR_NAME__` sidecar lifecycle.
//!
//! Generated from `templates/sidecar` — replace the `TODO` markers with the
//! engine-specific pieces (binary resolution, health probe, preset schema).
//! The contract tests in this module are part of the template: every
//! sidecar must satisfy spawn-lock, timeout-kill, and Drop-kill behavior.

pub mod lifecycle;
pub mod preset;

pub use lifecycle::{SidecarState, ensure_sidecar, reset_sidecar};
