//! Pluggable screen summary boundary (#168).

/// Host screen summarizer (V1: always unavailable).
#[derive(Debug, Clone, Copy)]
pub struct ScreenSummaryProvider;

impl ScreenSummaryProvider {
    /// V1 desktop build: no safe local summarizer is bundled.
    #[must_use]
    pub const fn v1_unavailable() -> Self {
        Self
    }

    /// Returns a short text summary when a provider is available.
    #[must_use]
    pub fn summarize(self) -> Option<String> {
        None
    }
}
