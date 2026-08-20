//! Caption overlay for streamed assistant text.

use crate::i18n;
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
    let max_width = (ctx.content_rect().width() * 0.72).clamp(240.0, 720.0);
    egui::Window::new(i18n::fl("caption-title"))
        .id(egui::Id::new("stage-caption"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
        .max_width(max_width)
        .show(ctx, |ui| {
            ui.set_max_width(max_width);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .font(egui::FontId::proportional(font_size.min(22.0)))
                        .color(egui::Color32::WHITE)
                        .strong(),
                )
                .wrap(),
            );
        });
}
