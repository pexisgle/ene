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

pub fn loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let loader = fluent_language_loader!();
        drop(loader.load_languages(&Localizations, &[loader.fallback_language().clone()]));
        loader
    })
}

pub fn select_language(lang: Language) {
    let loader = loader();
    let request_lang = match lang {
        Language::En => "en-US",
        Language::Ja => "ja",
    };
    if let Ok(lang_id) = request_lang.parse() {
        let requested_languages = vec![lang_id];
        let localizer = i18n_embed::DefaultLocalizer::new(loader, &Localizations);
        drop(localizer.select(&requested_languages));
    }
}

#[cfg(test)]
mod tests {
    use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
    use fluent_syntax::ast;

    use crate::settings_ui::provider_form::BUILTIN_PROVIDER_I18N_IDS;
    use crate::settings_ui::schema_form::BUILTIN_PROVIDER_GROUP_IDS;

    fn assert_catalog_parses(locale: &str, source: &str) {
        let errors = match FluentResource::try_new(source.to_string()) {
            Ok(_) => Vec::new(),
            Err((_, errors)) => errors,
        };
        assert!(
            errors.is_empty(),
            "{locale} Fluent catalog has parser errors: {errors:#?}"
        );
    }

    fn assert_catalog_resolves(locale: &str, source: &str, keys: &[String]) {
        let resource = FluentResource::try_new(source.to_string()).expect("catalog parses");
        let language: i18n_embed::unic_langid::LanguageIdentifier =
            locale.parse().expect("valid locale");
        let mut bundle = FluentBundle::new(vec![language]);
        bundle.add_resource(resource).expect("catalog loads");
        for key in keys {
            let message = bundle.get_message(key).expect("localized key exists");
            let pattern = message.value().expect("localized key has a value");
            let mut errors = Vec::new();
            let value = bundle.format_pattern(pattern, None, &mut errors);
            assert!(
                errors.is_empty(),
                "{locale} failed to format {key}: {errors:#?}"
            );
            assert!(!value.trim().is_empty(), "{locale} {key} is empty");
        }
    }

    fn assert_all_messages_resolve(locale: &str, source: &str) {
        let resource = FluentResource::try_new(source.to_string()).expect("catalog parses");
        let message_ids: Vec<String> = resource
            .entries()
            .filter_map(|entry| match entry {
                ast::Entry::Message(message) => Some(message.id.name.to_string()),
                _ => None,
            })
            .collect();
        let language: i18n_embed::unic_langid::LanguageIdentifier =
            locale.parse().expect("valid locale");
        let mut bundle = FluentBundle::new(vec![language]);
        bundle.add_resource(resource).expect("catalog loads");

        let variable_pattern =
            regex::Regex::new(r"\$\s*([A-Za-z][A-Za-z0-9_-]*)").expect("valid regex");
        let mut args = FluentArgs::new();
        for captures in variable_pattern.captures_iter(source) {
            args.set(captures[1].to_string(), 1_i64);
        }

        for id in message_ids {
            let message = bundle.get_message(&id).expect("catalog message exists");
            if let Some(pattern) = message.value() {
                let mut errors = Vec::new();
                let _formatted = bundle.format_pattern(pattern, Some(&args), &mut errors);
                assert!(
                    errors.is_empty(),
                    "{locale} failed to resolve {id}: {errors:#?}"
                );
            }
            for attribute in message.attributes() {
                let mut errors = Vec::new();
                let _formatted = bundle.format_pattern(attribute.value(), Some(&args), &mut errors);
                assert!(
                    errors.is_empty(),
                    "{locale} failed to resolve {id}.{}: {errors:#?}",
                    attribute.id()
                );
            }
        }
    }

    #[test]
    fn bundled_fluent_catalogs_parse() {
        assert_catalog_parses("en-US", include_str!("../i18n/en-US/ene_desktop.ftl"));
        assert_catalog_parses("ja", include_str!("../i18n/ja/ene_desktop.ftl"));
    }

    #[test]
    fn bundled_fluent_messages_resolve() {
        assert_all_messages_resolve("en-US", include_str!("../i18n/en-US/ene_desktop.ftl"));
        assert_all_messages_resolve("ja", include_str!("../i18n/ja/ene_desktop.ftl"));
    }

    #[test]
    fn builtin_provider_selector_and_group_keys_resolve_in_every_locale() {
        let mut keys = Vec::new();
        for (_, i18n_id) in BUILTIN_PROVIDER_I18N_IDS {
            keys.push(format!("provider-selector-{i18n_id}-label"));
            keys.push(format!("provider-selector-{i18n_id}-desc"));
        }
        keys.extend(
            BUILTIN_PROVIDER_GROUP_IDS
                .iter()
                .map(|group| format!("provider-group-{group}")),
        );
        assert_catalog_resolves(
            "en-US",
            include_str!("../i18n/en-US/ene_desktop.ftl"),
            &keys,
        );
        assert_catalog_resolves("ja", include_str!("../i18n/ja/ene_desktop.ftl"), &keys);
    }
}
