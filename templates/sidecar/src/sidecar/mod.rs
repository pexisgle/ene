//! Managed `__SIDECAR_NAME__` sidecar lifecycle.
//!
//! Generated from `templates/sidecar`. The generic pieces (config shape,
//! binary resolution, preset writer, spawn → health-check → kill lifecycle)
//! ship complete; only engine-specific details — the health probe URL and
//! preset schema — need adapting. The contract tests in this module are part
//! of the template: every sidecar must satisfy spawn-lock, timeout-kill, and
//! Drop-kill behavior.

pub mod config;
pub mod lifecycle;
pub mod preset;

pub use lifecycle::{SidecarState, ensure_sidecar, reset_sidecar};
