//! OS overlay surface.
//!
//! wgpu is owned by [`crate::render`]. This module only decides how (or
//! whether) a transparent OS window exists. KDE Wayland layer-shell and
//! Windows DWM are compiled stubs: selecting them is 未実施.

mod dwm;
mod headless;
mod wayland;

pub use dwm::windows_dwm_probe;
pub use headless::HeadlessOverlay;
pub use wayland::kde_layer_shell_probe;

use crate::ipc::{LocalUiFact, OverlayKind, PlacementBox};

/// Outcome of asking for a real OS overlay. Never pretended as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayProbe {
    /// Recorded as 未実施. Do not treat a headless process as Wayland/DWM evidence.
    NotRun,
}

/// Overlay in this process. Production always opens headless until a probe
/// adopts an OS backend.
#[derive(Debug)]
pub enum Overlay {
    Headless(HeadlessOverlay),
}

impl Overlay {
    #[must_use]
    pub fn open() -> Self {
        Self::Headless(HeadlessOverlay::new())
    }

    #[must_use]
    pub fn kind(&self) -> OverlayKind {
        match self {
            Self::Headless(_) => OverlayKind::Headless,
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        match self {
            Self::Headless(inner) => inner.set_visible(visible),
        }
    }

    #[must_use]
    pub fn visible(&self) -> bool {
        match self {
            Self::Headless(inner) => inner.visible(),
        }
    }

    pub fn set_placement(&mut self, placement: PlacementBox) {
        match self {
            Self::Headless(inner) => inner.set_placement(placement),
        }
    }

    #[must_use]
    pub fn placement(&self) -> PlacementBox {
        match self {
            Self::Headless(inner) => inner.placement(),
        }
    }

    /// Local UI facts exist so a future OS backend can report drag/resize/hide
    /// without talking to Host. Headless never synthesizes them.
    #[must_use]
    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        match self {
            Self::Headless(inner) => inner.take_local_ui(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Overlay, OverlayProbe, kde_layer_shell_probe, windows_dwm_probe};
    use crate::ipc::{LocalUiFact, OverlayKind, PlacementBox};

    #[test]
    fn production_overlay_is_headless_and_probes_are_not_run() {
        let overlay = Overlay::open();
        assert_eq!(overlay.kind(), OverlayKind::Headless);
        assert!(!overlay.visible());
        assert_eq!(kde_layer_shell_probe(), OverlayProbe::NotRun);
        assert_eq!(windows_dwm_probe(), OverlayProbe::NotRun);
    }

    #[test]
    fn headless_records_show_hide_and_placement() {
        let mut overlay = Overlay::open();
        overlay.set_visible(true);
        overlay.set_placement(PlacementBox {
            x: 8,
            y: 16,
            width: 100,
            height: 200,
            scale: 1.5,
        });
        assert!(overlay.visible());
        assert_eq!(overlay.placement().width, 100);
        overlay.set_visible(false);
        assert!(!overlay.visible());
        assert_eq!(overlay.take_local_ui(), None);
    }

    #[test]
    fn headless_can_queue_local_ui_for_contract_tests() {
        let mut overlay = Overlay::open();
        match &mut overlay {
            Overlay::Headless(inner) => inner.push_local_ui(LocalUiFact::Hide),
        }
        assert_eq!(overlay.take_local_ui(), Some(LocalUiFact::Hide));
        assert_eq!(overlay.take_local_ui(), None);
    }
}
