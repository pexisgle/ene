use enigo::Key;

/// Shared helper to parse key names into Enigo Key enums.
pub fn parse_key(name: &str) -> Option<Key> {
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => Some(Key::Control),
        "shift" => Some(Key::Shift),
        "alt" | "option" => Some(Key::Alt),
        "meta" | "super" | "cmd" | "command" | "win" | "windows" => Some(Key::Meta),
        "return" | "enter" => Some(Key::Return),
        "escape" | "esc" => Some(Key::Escape),
        "tab" => Some(Key::Tab),
        "space" => Some(Key::Space),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "page_up" => Some(Key::PageUp),
        "pagedown" | "page_down" => Some(Key::PageDown),
        "up" | "arrow_up" => Some(Key::UpArrow),
        "down" | "arrow_down" => Some(Key::DownArrow),
        "left" | "arrow_left" => Some(Key::LeftArrow),
        "right" | "arrow_right" => Some(Key::RightArrow),
        "insert" | "ins" => Some(Key::Insert),
        "capslock" | "caps_lock" => Some(Key::CapsLock),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        s => {
            let mut chars = s.chars();
            if let Some(c) = chars.next()
                && chars.next().is_none()
            {
                Some(Key::Unicode(c))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_modifiers_and_arrows() {
        assert!(matches!(parse_key("ctrl"), Some(Key::Control)));
        assert!(matches!(parse_key("shift"), Some(Key::Shift)));
        assert!(matches!(parse_key("alt"), Some(Key::Alt)));
        assert!(matches!(parse_key("meta"), Some(Key::Meta)));
        assert!(matches!(parse_key("return"), Some(Key::Return)));
        assert!(matches!(parse_key("enter"), Some(Key::Return)));
        assert!(matches!(parse_key("escape"), Some(Key::Escape)));
        assert!(matches!(parse_key("esc"), Some(Key::Escape)));
        assert!(matches!(parse_key("tab"), Some(Key::Tab)));
        assert!(matches!(parse_key("space"), Some(Key::Space)));
        assert!(matches!(parse_key("up"), Some(Key::UpArrow)));
        assert!(matches!(parse_key("down"), Some(Key::DownArrow)));
        assert!(matches!(parse_key("left"), Some(Key::LeftArrow)));
        assert!(matches!(parse_key("right"), Some(Key::RightArrow)));
        for n in 1..=12 {
            assert!(parse_key(&format!("f{n}")).is_some(), "f{n} should parse");
        }
    }

    #[test]
    fn parses_ascii_single_char() {
        assert!(matches!(parse_key("a"), Some(Key::Unicode('a'))));
        assert!(matches!(parse_key("Z"), Some(Key::Unicode('z'))));
        assert!(matches!(parse_key("5"), Some(Key::Unicode('5'))));
    }

    #[test]
    fn parses_non_ascii_single_char() {
        assert!(matches!(parse_key("é"), Some(Key::Unicode('é'))));
        assert!(matches!(parse_key("日"), Some(Key::Unicode('日'))));
        assert!(matches!(parse_key("ñ"), Some(Key::Unicode('ñ'))));
    }

    #[test]
    fn rejects_unknown_names() {
        assert!(parse_key("foobar").is_none());
        assert!(parse_key("").is_none());
    }

    #[test]
    fn rejects_multi_char_input() {
        assert!(parse_key("ab").is_none());
        assert!(parse_key("ctrlx").is_none());
    }
}
