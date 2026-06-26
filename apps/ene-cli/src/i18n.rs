use i18n_embed::{
    LanguageLoader, Localizer,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// Returns the static language loader for CLI.
pub fn loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let loader = fluent_language_loader!();
        let _ = loader.load_languages(&Localizations, &[loader.fallback_language().clone()]);

        // Negotiate language with system locale on startup
        let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
        let localizer = i18n_embed::DefaultLocalizer::new(&loader, &Localizations);
        let _ = localizer.select(&requested_languages);

        loader
    })
}
