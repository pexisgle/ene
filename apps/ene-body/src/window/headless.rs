//! In-process overlay that records visibility and the placement box.
//!
//! Used when no OS compositor backend has been probe-adopted. Hide here is
//! still not Companion stop.

use crate::ipc::PlacementBox;

#[derive(Debug)]
pub struct HeadlessOverlay {
    visible: bool,
    placement: PlacementBox,
    unavailable_reason: Option<String>,
}

impl HeadlessOverlay {
    #[must_use]
    pub fn unavailable(reason: String) -> Self {
        Self {
            visible: false,
            placement: PlacementBox::default(),
            unavailable_reason: Some(reason),
        }
    }

    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable_reason.as_deref()
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
}
