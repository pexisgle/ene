use crate::settings::Language;
use i18n_embed::{
    LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// Returns the global static Fluent language loader.
pub fn loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let loader = fluent_language_loader!();
        // Load default fallback language
        let _ = loader.load_languages(&Localizations, &[loader.fallback_language().clone()]);
        loader
    })
}

/// Dynamically changes the active Fluent language catalog.
pub fn select_language(lang: Language) {
    let loader = loader();
    let request_lang = match lang {
        Language::En => "en-US",
        Language::Ja => "ja",
    };
    if let Ok(lang_id) = request_lang.parse() {
        let requested_languages = vec![lang_id];
        let localizer = i18n_embed::DefaultLocalizer::new(loader, &Localizations);
        let _ = localizer.select(&requested_languages);
    }
}
