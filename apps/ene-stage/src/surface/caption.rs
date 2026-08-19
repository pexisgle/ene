//! Caption overlay for streamed assistant text.

use eframe::egui::{self, Align2, Color32, FontId, RichText};
use crate::surface::SurfaceUiState;

pub fn show(ctx: &egui::Context, state: &SurfaceUiState, font_size: f32) {
    if state.caption.is_empty() && state.streaming_text.is_empty() {
        return;
    }
    let text = if state.caption.is_empty() {
        state.streaming_text.as_str()
    } else {
        state.caption.as_str()
    };
    egui::Area::new(egui::Id::new("stage-caption"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -48.0])
        .show(ctx, |ui| {
            ui.label(
                RichText::new(text)
                    .font(FontId::proportional(font_size))
                    .color(Color32::WHITE)
                    .strong(),
            );
        });
}
