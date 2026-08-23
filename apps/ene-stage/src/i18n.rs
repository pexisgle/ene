use std::collections::HashSet;
use std::sync::OnceLock;

use i18n_embed::DesktopLanguageRequester;
use i18n_embed::LanguageLoader;
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use parking_lot::{Mutex, RwLock};
use rust_embed::RustEmbed;
use unic_langid::LanguageIdentifier;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LOADER: OnceLock<RwLock<FluentLanguageLoader>> = OnceLock::new();
static MISSING_KEYS: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();

fn loader_lock() -> &'static RwLock<FluentLanguageLoader> {
    LOADER.get_or_init(|| {
        let language_loader = fluent_language_loader!();
        let requested = DesktopLanguageRequester::requested_languages();
        let _selected = i18n_embed::select(&language_loader, &Localizations, &requested);
        RwLock::new(language_loader)
    })
}

fn should_report_missing(language: &str, key: &str) -> bool {
    MISSING_KEYS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .insert((language.to_owned(), key.to_owned()))
}

/// Apply a BCP-47 tag (`en-US`, `ja`). Empty string follows the OS default.
pub fn select_language(tag: &str) {
    let loader = loader_lock();
    let guard = loader.read();
    if tag.is_empty() {
        let requested = DesktopLanguageRequester::requested_languages();
        let _selected = i18n_embed::select(&*guard, &Localizations, &requested);
        return;
    }
    let Ok(lang): Result<LanguageIdentifier, _> = tag.parse() else {
        tracing::warn!(tag, "unrecognised language tag");
        return;
    };
    let _selected = i18n_embed::select(&*guard, &Localizations, &[lang]);
}

#[must_use]
pub fn fl(key: &str) -> String {
    let loader = loader_lock();
    let guard = loader.read();
    let value = guard.get(key);
    if !guard.has(key) {
        let language = guard.current_languages().first().map_or_else(
            || guard.fallback_language().to_string(),
            ToString::to_string,
        );
        if should_report_missing(&language, key) {
            tracing::warn!(key, language = %language, "localization key is missing");
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fl_resolves_app_title() {
        let title = fl("app-title");
        assert!(!title.is_empty());
        assert_ne!(title, "app-title");
    }

    #[test]
    fn en_and_ja_ftl_have_the_same_keys() {
        let en = include_str!("../i18n/en-US/stage.ftl");
        let ja = include_str!("../i18n/ja/stage.ftl");
        fn keys(src: &str) -> Vec<&str> {
            src.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        return None;
                    }
                    line.split_once('=').map(|(key, _)| key.trim())
                })
                .collect()
        }
        assert_eq!(keys(en), keys(ja));
    }

    #[test]
    fn missing_key_diagnostics_are_deduplicated_by_language_and_key() {
        assert!(should_report_missing("ja", "test-missing-key"));
        assert!(!should_report_missing("ja", "test-missing-key"));
        assert!(should_report_missing("ja", "another-missing-key"));
        assert!(should_report_missing("en-US", "test-missing-key"));
    }
}
