fn loader() -> &'static i18n_embed::fluent::FluentLanguageLoader {
    static LOADER: std::sync::OnceLock<i18n_embed::fluent::FluentLanguageLoader> =
        std::sync::OnceLock::new();
    LOADER.get_or_init(|| {
        i18n_embed::fluent::FluentLanguageLoader::new("test", "en-US".parse().unwrap())
    })
}

mod i18n {
    use super::loader;
    ene_i18n_macros::i18n_keys!(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/i18n/en-US/test.ftl"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/i18n/ja/test.ftl")
    );
}

fn main() {
    // No-arg message → fn() -> String
    let _: String = i18n::hello();
    // Single-param message → fn(impl Into<FluentValue>) -> String
    let _: String = i18n::greeting("world");
    // Two-param message
    let _: String = i18n::two_params("a", "b");
    // Keyword sanitization: FTL key `type` → Rust `r#type`
    let _: String = i18n::r#type("foo");
}
