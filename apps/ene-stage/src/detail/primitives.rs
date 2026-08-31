//! Toolkit-free Detail status cards used by Slint projection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusTone {
    Ready,
    NeedsConfig,
    Error,
}

/// Uniform Ready / Needs config / Error row with an optional next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusCard {
    pub state: StatusTone,
    pub title: String,
    pub summary: String,
    pub action_label: Option<String>,
}
