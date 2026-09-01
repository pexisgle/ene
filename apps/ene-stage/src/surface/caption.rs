//! Caption overlay for streamed assistant text.

use crate::surface::SurfaceUiState;

#[cfg(test)]
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

/// True when `text` is spoken reply content, not a kernel provider-failure marker.
#[must_use]
pub(crate) fn is_speech_caption(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && !trimmed
            .to_ascii_lowercase()
            .starts_with("the chat provider failed")
}

#[must_use]
pub fn speech_text(state: &SurfaceUiState) -> &str {
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
    fn all_named_positions_are_handled() {
        for position in POSITIONS {
            let (x, y) = outer_offset(position, (1920, 1080), (720, 160));
            assert!(x < 1920);
            assert!(y < 1080);
        }
    }

    #[test]
    fn top_sits_near_the_monitor_top() {
        let (x, y) = outer_offset("top", (1920, 1080), (720, 160));
        assert_eq!(x, (1920 - 720) / 2);
        assert_eq!(y, 48);
    }

    #[test]
    fn bottom_is_the_default_and_unknown_fallback() {
        let bottom = outer_offset("bottom", (1920, 1080), (720, 160));
        let unknown = outer_offset("elsewhere", (1920, 1080), (720, 160));
        assert_eq!(bottom, unknown);
        assert_eq!(bottom.1, 1080 - 160 - 96);
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
        assert!(is_speech_caption("401 Unauthorized"));
        assert!(is_speech_caption("Error handling in Rust"));
        assert!(is_speech_caption("Hello from the companion."));
    }

    #[test]
    fn speech_text_prefers_caption_and_hides_provider_errors() {
        let mut state = SurfaceUiState::default();
        assert_eq!(speech_text(&state), "");
        state.streaming_text = "hello".into();
        assert_eq!(speech_text(&state), "hello");
        state.caption = "spoken".into();
        assert_eq!(speech_text(&state), "spoken");
        state.caption = "The chat provider failed: boom".into();
        assert_eq!(speech_text(&state), "");
    }
}
