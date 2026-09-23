//! In-process overlay for when no OS compositor backend has been probe-adopted.
//!
//! Its only output is the explicit unavailability reason; hide here is still
//! not Companion stop.

#[derive(Debug)]
pub struct HeadlessOverlay {
    reason: String,
}

impl HeadlessOverlay {
    #[must_use]
    pub fn unavailable(reason: String) -> Self {
        Self { reason }
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}
