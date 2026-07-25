//! Lightweight heuristics for detecting code / terminal windows and
//! extracting rough text hints from captured screen regions (#215).
//!
//! This module does **not** perform full OCR. The `is_code_window` function is
//! a cheap window-title/class-based filter that lets the observer signal to the
//! vision model that it is looking at code-like content. Full offline OCR
//! (e.g. via Tesseract or a local OCR model) would go here in a future iteration.

use image::DynamicImage;

/// Determine whether the active window *likely* contains code or structured
/// terminal output by checking the window title and class against a list of
/// known developer-tool substrings.
///
/// Returns `true` when the combined `window_title` and `window_class` text
/// contain any of the known code/terminal indicators.
///
/// # Example
///
/// ```
/// # use ene_desktop::proactive_observe::ocr::is_code_window;
/// assert!(is_code_window("main.rs - VSCode", "code"));
/// assert!(is_code_window("bash", "xterm"));
/// assert!(!is_code_window("Firefox", "firefox"));
/// ```
pub fn is_code_window(window_title: &str, window_class: &str) -> bool {
    let lower = format!("{} {}", window_title, window_class).to_lowercase();
    let code_indicators = [
        "code",
        "editor",
        "terminal",
        "console",
        "vim",
        "nvim",
        "emacs",
        "vscode",
        "intellij",
        "ide",
        "bash",
        "zsh",
        "fish",
        "powershell",
        "cmd",
        "git",
        "diff",
        "log",
    ];
    code_indicators.iter().any(|ind| lower.contains(ind))
}

/// Extract ASCII text-line hints from a raw RGB image (simplified heuristic).
///
/// This is a **placeholder** for future local OCR integration. The current
/// implementation always returns `None`, indicating that no text could be
/// extracted. When integrated with a local OCR engine (e.g. Tesseract, a
/// small ONNX model, or the built-in Gemma vision model with a "read this
/// text" prompt), this function would return extracted text lines.
///
/// The function signature is stabilised so callers can be written today and
/// filled in later.
pub fn extract_text_hints(_img: &DynamicImage) -> Option<String> {
    // Placeholder: full OCR requires tesseract or similar.
    // For now, return None to indicate "could not extract".
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vscode() {
        assert!(is_code_window("main.rs - VSCode", "code"));
    }

    #[test]
    fn detects_terminal() {
        assert!(is_code_window("bash", "xterm"));
        assert!(is_code_window("zsh", "Alacritty"));
        assert!(is_code_window("", "WindowsTerminal"));
    }

    #[test]
    fn detects_ide() {
        assert!(is_code_window("project - IntelliJ IDEA", "jetbrains-idea"));
        assert!(is_code_window("untitled - Emacs", "Emacs"));
    }

    #[test]
    fn rejects_browser() {
        assert!(!is_code_window("Firefox", "firefox"));
        assert!(!is_code_window("", "google-chrome"));
    }

    #[test]
    fn rejects_media_player() {
        assert!(!is_code_window("Spotify", "spotify"));
        assert!(!is_code_window("VLC media player", "vlc"));
    }

    #[test]
    fn extract_text_placeholder_returns_none() {
        let img = DynamicImage::new_rgba8(100, 100);
        assert!(extract_text_hints(&img).is_none());
    }
}
