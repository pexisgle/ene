//! Caption overlay for streamed assistant text.

use crate::i18n;
use crate::surface::SurfaceUiState;

/// True when `text` is spoken reply content, not a provider or HTTP error body.
#[must_use]
pub(crate) fn is_speech_caption(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("the chat provider failed") {
        return false;
    }
    if lower.starts_with("error:") || lower.starts_with("error ") {
        return false;
    }
    if lower.contains("401 unauthorized") || lower.contains("403 forbidden") {
        return false;
    }
    true
}

pub fn show(ctx: &egui::Context, state: &SurfaceUiState, font_size: f32) {
    let text = speech_text(state);
    if text.is_empty() {
        return;
    }
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

fn speech_text(state: &SurfaceUiState) -> &str {
    let candidate = if state.caption.is_empty() {
        state.streaming_text.as_str()
    } else {
        state.caption.as_str()
    };
    if is_speech_caption(candidate) {
        candidate
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::is_speech_caption;

    #[test]
    fn provider_errors_are_not_speech() {
        assert!(!is_speech_caption(
            "The chat provider failed: 401 Unauthorized"
        ));
        assert!(!is_speech_caption("401 Unauthorized"));
        assert!(!is_speech_caption("error: model not found"));
        assert!(is_speech_caption("Hello from the companion."));
    }
}
