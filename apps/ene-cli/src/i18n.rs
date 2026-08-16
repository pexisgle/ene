use i18n_embed::{
    LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// The loader negotiates the active language with the system locale. Use
/// [`select_language`] to override the negotiated language (e.g. from the
/// `--lang` CLI flag).
pub fn loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let loader = fluent_language_loader!();
        drop(loader.load_languages(&Localizations, &[loader.fallback_language().clone()]));

        let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
        let localizer = i18n_embed::DefaultLocalizer::new(&loader, &Localizations);
        drop(localizer.select(&requested_languages));

        loader
    })
}

/// Accepts `en`/`en-US` for English and `ja` for Japanese; any other value
/// falls back to the system-negotiated language without changing it.
pub fn select_language(lang: &str) {
    let request_lang = match lang.trim().to_ascii_lowercase().as_str() {
        "en" | "en-us" => "en-US",
        "ja" | "ja-jp" => "ja",
        _ => return,
    };
    if let Ok(lang_id) = request_lang.parse() {
        let requested_languages = vec![lang_id];
        let localizer = i18n_embed::DefaultLocalizer::new(loader(), &Localizations);
        drop(localizer.select(&requested_languages));
    }
}

/// Primary language code of the active UI locale: the `--lang` override when
/// set, else the system-negotiated locale.
///
/// Drives character card localization, so a Japanese UI selects
/// `character.ja.json` diffs without extra configuration.
pub fn active_language_code() -> String {
    loader().current_languages().first().map_or_else(
        || ene_config::system_language().to_string(),
        |lang| lang.language.as_str().to_string(),
    )
}
