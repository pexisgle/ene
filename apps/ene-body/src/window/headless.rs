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
