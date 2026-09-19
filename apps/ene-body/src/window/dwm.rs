//! Windows layered/DWM overlay backend (provisional).
//!
//! Intended path: layered window + DWM transparency and non-rectangular
//! hit-test. This module compiles on every target so Linux CI still type-
//! checks the probe record; it does not call Win32.
//!
//! This Cloud Agent has no Windows 11 desktop. Probe status: 未実施.

use super::OverlayProbe;

/// Returns [`OverlayProbe::NotRun`]. Does not create a layered window.
#[must_use]
pub fn windows_dwm_probe() -> OverlayProbe {
    OverlayProbe::NotRun
}
