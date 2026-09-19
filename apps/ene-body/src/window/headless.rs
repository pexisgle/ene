//! In-process overlay that records visibility and the placement box.
//!
//! Used when no OS compositor backend has been probe-adopted. Hide here is
//! still not Companion stop.

use crate::ipc::{LocalUiFact, PlacementBox};

#[derive(Debug)]
pub struct HeadlessOverlay {
    visible: bool,
    placement: PlacementBox,
    local_ui: Option<LocalUiFact>,
}

impl HeadlessOverlay {
    #[must_use]
    pub fn new() -> Self {
        Self {
            visible: false,
            placement: PlacementBox::default(),
            local_ui: None,
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn set_placement(&mut self, placement: PlacementBox) {
        self.placement = placement;
    }

    #[must_use]
    pub fn placement(&self) -> PlacementBox {
        self.placement
    }

    pub fn push_local_ui(&mut self, fact: LocalUiFact) {
        self.local_ui = Some(fact);
    }

    #[must_use]
    pub fn take_local_ui(&mut self) -> Option<LocalUiFact> {
        self.local_ui.take()
    }
}

impl Default for HeadlessOverlay {
    fn default() -> Self {
        Self::new()
    }
}
