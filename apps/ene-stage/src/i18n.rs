use std::sync::OnceLock;

use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::DesktopLanguageRequester;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();

fn loader_inner() -> &'static FluentLanguageLoader {
    LOADER.get_or_init(|| {
        let language_loader = fluent_language_loader!();
        let requested_languages = DesktopLanguageRequester::requested_languages();
        let _selected = i18n_embed::select(&language_loader, &Localizations, &requested_languages);
        language_loader
    })
}

#[must_use]
pub fn loader() -> &'static FluentLanguageLoader {
    loader_inner()
}

#[must_use]
pub fn fl(key: &str) -> String {
    loader().get(key)
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
}
