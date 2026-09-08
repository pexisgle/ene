//! Data-only user theme overlay. Invalid files fall back to defaults.

use std::path::Path;

use serde::Deserialize;

use crate::ui::EneTheme;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserThemeFile {
    #[serde(default)]
    pub dark: Option<bool>,
    #[serde(default)]
    pub reduced_motion: Option<bool>,
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub fg: Option<String>,
    #[serde(default)]
    pub accent: Option<String>,
}

#[must_use]
pub fn load_user_theme(path: &Path) -> UserThemeFile {
    let Ok(text) = std::fs::read_to_string(path) else {
        return UserThemeFile::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn apply_to_global(theme: &EneTheme<'_>, file: &UserThemeFile) {
    if let Some(dark) = file.dark {
        theme.set_dark(dark);
    }
    if let Some(reduced) = file.reduced_motion {
        theme.set_reduced_motion(reduced);
    }
    if let Some(color) = file.bg.as_deref().and_then(parse_color) {
        theme.set_bg(color);
    }
    if let Some(color) = file.fg.as_deref().and_then(parse_color) {
        theme.set_fg(color);
    }
    if let Some(color) = file.accent.as_deref().and_then(parse_color) {
        theme.set_accent(color);
    }
}

fn parse_color(value: &str) -> Option<slint::Color> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_toml_falls_back() {
        let parsed: UserThemeFile = toml::from_str("not = [").unwrap_or_default();
        assert!(parsed.accent.is_none());
    }

    #[test]
    fn hex_color_parses() {
        assert!(parse_color("#7eb8ff").is_some());
        assert!(parse_color("zzzzzz").is_none());
    }
}
