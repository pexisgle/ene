//! Lightweight heuristics for detecting code / terminal windows and
//! extracting rough text hints from captured screen regions.
//!
//! This module does **not** perform OCR itself. The `is_code_window` function
//! is a cheap window-title/class-based filter that decides whether the
//! observer pays for a verbatim transcription pass: code-like windows get a
//! `read_screen_text` call against the local vision model, and the resulting
//! text rides along to the summary prompt as an `ocr_text` hint.

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
    let lower = format!("{window_title} {window_class}").to_lowercase();
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
}
