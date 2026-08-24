//! Shared Detail panel primitives: explained headings, actionable empty
//! states, status cards, safety hints, and collapsed raw identifiers.

use egui::{Color32, CornerRadius, Frame, Margin, RichText};

/// Heading plus the one sentence a beginner needs to parse the section.
pub(crate) struct SectionHeading {
    pub title: String,
    pub help: String,
}

impl SectionHeading {
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        ui.heading(&self.title);
        if !self.help.is_empty() {
            ui.weak(&self.help);
        }
    }
}

/// Panel placeholder that names the next step instead of staying blank.
pub(crate) struct EmptyState {
    pub title: String,
    pub explanation: String,
    pub action_label: Option<String>,
}

impl EmptyState {
    /// Returns true when the primary action was pressed this frame.
    pub(crate) fn show(&self, ui: &mut egui::Ui) -> bool {
        ui.add_space(4.0);
        let mut clicked = false;
        Frame::new()
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(10))
            .stroke((1.0, ui.visuals().weak_text_color()))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.strong(RichText::new(&self.title));
                    if !self.explanation.is_empty() {
                        ui.label(&self.explanation);
                    }
                    if let Some(label) = &self.action_label
                        && ui.button(label).clicked()
                    {
                        clicked = true;
                    }
                });
            });
        ui.add_space(4.0);
        clicked
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusTone {
    Ready,
    NeedsConfig,
    Error,
}

/// Uniform Ready / Needs config / Error row with an optional next step.
pub(crate) struct StatusCard {
    pub state: StatusTone,
    pub title: String,
    pub summary: String,
    pub action_label: Option<String>,
}

impl StatusCard {
    fn tone_color(&self, dark: bool) -> Color32 {
        match self.state {
            StatusTone::Ready => Color32::LIGHT_GREEN,
            StatusTone::NeedsConfig => Color32::YELLOW,
            StatusTone::Error => Color32::LIGHT_RED,
        }
        .gamma_multiply(if dark { 0.85 } else { 0.7 })
    }

    /// Returns true when the action button was pressed this frame.
    pub(crate) fn show(&self, ui: &mut egui::Ui) -> bool {
        let mut clicked = false;
        let tone = self.tone_color(ui.visuals().dark_mode);
        Frame::new()
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    if !self.title.is_empty() {
                        ui.strong(RichText::new(&self.title));
                    }
                    ui.colored_label(tone, RichText::new(&self.summary));
                    if let Some(label) = &self.action_label
                        && ui.button(label).clicked()
                    {
                        clicked = true;
                    }
                });
            });
        clicked
    }
}

/// Safety callout for modes that remove guardrails (auto approval, shared data).
pub(crate) fn danger_hint(ui: &mut egui::Ui, text: &str) {
    let dark = ui.visuals().dark_mode;
    let (bg, fg) = if dark {
        (
            Color32::from_rgb(88, 66, 26),
            Color32::from_rgb(235, 195, 110),
        )
    } else {
        (
            Color32::from_rgb(250, 236, 196),
            Color32::from_rgb(125, 82, 12),
        )
    };
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.colored_label(fg, RichText::new(text));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_tones_are_distinct() {
        assert_ne!(StatusTone::Ready, StatusTone::NeedsConfig);
        assert_ne!(StatusTone::NeedsConfig, StatusTone::Error);
        assert_ne!(StatusTone::Ready, StatusTone::Error);
    }
}
