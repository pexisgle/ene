//! Caption overlay for streamed assistant text.

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
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .font(egui::FontId::proportional(font_size))
                    .color(egui::Color32::WHITE)
                    .strong(),
            );
        });
}
