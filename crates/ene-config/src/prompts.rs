//! Locale codes shared by pattern packs and character cards.

/// Language codes that ship a compile-time embedded pattern pack.
pub const SUPPORTED_LANGUAGES: &[&str] = &["en", "ja"];

/// Resolves a free-form language tag to the directory code used under
/// `assets/lang/`.
///
/// Matching is case-insensitive and keeps only the primary subtag, so `"ja"`,
/// `"JA"`, and `"ja-JP"` all resolve to `"ja"`. The legacy alias `"jp"` also
/// maps to `"ja"`. Empty input and any primary subtag that is not ASCII
/// alphabetic fall back to `"en"` so the result is safe to join into a path.
pub fn resolve_language_alias(lang: &str) -> String {
    let primary = lang.split(['-', '_']).next().unwrap_or_default();
    let lower = primary.to_ascii_lowercase();
    if lower.is_empty() || !lower.chars().all(|c| c.is_ascii_alphabetic()) {
        return "en".to_string();
    }
    if lower == "jp" {
        "ja".to_string()
    } else {
        lower
    }
}

/// System-locale default for an unset app-language setting, resolved once and
/// cached: a primary language code of `ja` selects Japanese, everything else
/// (including absent locale, `C.UTF-8`, `en-US`, `fr`) falls back to English
/// so CI stays deterministic.
static SYSTEM_LANGUAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Resolves the app-wide default language from the OS locale, cached on first
/// use.
pub fn system_language() -> &'static str {
    SYSTEM_LANGUAGE
        .get_or_init(|| resolve_system_language(sys_locale::get_locale().as_deref()))
        .as_str()
}

/// Maps an optional OS locale string to the app-wide default language.
///
/// Only a primary subtag of `ja` selects Japanese; every other value keeps the
/// English default. Kept pure so tests can pin the locale without touching
/// process-global environment variables.
pub fn resolve_system_language(locale: Option<&str>) -> String {
    match locale.map(resolve_language_alias).as_deref() {
        Some("ja") => "ja".to_string(),
        _ => "en".to_string(),
    }
}

pub(crate) fn is_embedded_language(code: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&code)
}

#[cfg(test)]
mod tests {
    use super::{resolve_language_alias, resolve_system_language};

    #[test]
    fn resolve_language_alias_rejects_non_ascii_alphabetic() {
        assert_eq!(resolve_language_alias(""), "en");
        assert_eq!(resolve_language_alias("../../../tmp/evil"), "en");
        assert_eq!(resolve_language_alias("ja/../evil"), "en");
        assert_eq!(resolve_language_alias("en2"), "en");
        assert_eq!(resolve_language_alias("日本語"), "en");
        assert_eq!(resolve_language_alias("ja"), "ja");
        assert_eq!(resolve_language_alias("jp"), "ja");
        assert_eq!(resolve_language_alias("en-US"), "en");
    }

    #[test]
    fn resolve_system_language_selects_ja_only_for_japanese_locale() {
        assert_eq!(resolve_system_language(Some("ja_JP.UTF-8")), "ja");
        assert_eq!(resolve_system_language(Some("JA")), "ja");
        assert_eq!(resolve_system_language(Some("ja-JP")), "ja");
        assert_eq!(resolve_system_language(Some("en-US")), "en");
        assert_eq!(resolve_system_language(Some("fr_FR.UTF-8")), "en");
        assert_eq!(resolve_system_language(Some("C.UTF-8")), "en");
        assert_eq!(resolve_system_language(None), "en");
    }
}
