//! Caption overlay for streamed assistant text.

use crate::i18n;
use crate::surface::SurfaceUiState;

pub const POSITIONS: [&str; 4] = ["bottom", "top", "left", "right"];

/// Native-window offset inside a monitor, in physical pixels from the top-left.
#[must_use]
pub fn outer_offset(position: &str, screen: (u32, u32), inner: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = screen;
    let (iw, ih) = inner;
    match position {
        "top" => (sw.saturating_sub(iw) / 2, 48),
        "left" => (24, sh.saturating_sub(ih) / 2),
        "right" => (
            sw.saturating_sub(iw).saturating_sub(24),
            sh.saturating_sub(ih) / 2,
        ),
        _ => (
            sw.saturating_sub(iw) / 2,
            sh.saturating_sub(ih.saturating_add(96)),
        ),
    }
}

#[must_use]
pub fn egui_anchor(position: &str) -> (egui::Align2, [f32; 2]) {
    match position {
        "top" => (egui::Align2::CENTER_TOP, [0.0, 16.0]),
        "left" => (egui::Align2::LEFT_CENTER, [16.0, 0.0]),
        "right" => (egui::Align2::RIGHT_CENTER, [-16.0, 0.0]),
        _ => (egui::Align2::CENTER_BOTTOM, [0.0, -16.0]),
    }
}

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

pub fn show(
    ctx: &egui::Context,
    state: &SurfaceUiState,
    font_size: f32,
    position: &str,
    pinned: bool,
) {
    let text = speech_text(state);
    if text.is_empty() {
        return;
    }
    let max_width = (ctx.content_rect().width() * 0.72).clamp(240.0, 720.0);
    let (anchor, offset) = egui_anchor(position);
    egui::Window::new(i18n::fl("caption-title"))
        .id(egui::Id::new("stage-caption"))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .movable(!pinned)
        .anchor(anchor, offset)
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
    use super::*;

    #[test]
    fn top_sits_near_the_monitor_top() {
        let (x, y) = outer_offset("top", (1920, 1080), (720, 160));
        assert_eq!(x, (1920 - 720) / 2);
        assert_eq!(y, 48);
        assert_eq!(egui_anchor("top").0, egui::Align2::CENTER_TOP);
    }

    #[test]
    fn bottom_is_the_default_and_unknown_fallback() {
        let bottom = outer_offset("bottom", (1920, 1080), (720, 160));
        let unknown = outer_offset("elsewhere", (1920, 1080), (720, 160));
        assert_eq!(bottom, unknown);
        assert_eq!(bottom.1, 1080 - 160 - 96);
        assert_eq!(egui_anchor("bottom").0, egui::Align2::CENTER_BOTTOM);
    }

    #[test]
    fn left_and_right_are_vertically_centered() {
        let left = outer_offset("left", (1920, 1080), (720, 160));
        let right = outer_offset("right", (1920, 1080), (720, 160));
        assert_eq!(left.0, 24);
        assert_eq!(right.0, 1920 - 720 - 24);
        assert_eq!(left.1, (1080 - 160) / 2);
        assert_eq!(right.1, left.1);
    }

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
