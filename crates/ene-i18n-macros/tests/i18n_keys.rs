//! Integration test verifying that `i18n_keys!` generates correct, compilable
//! accessor functions from `.ftl` catalogs.
//!
//! This runs as a normal integration test (not trybuild) so that
//! `CARGO_MANIFEST_DIR` points to the crate root and the relative FTL paths
//! resolve correctly.
#![expect(
    clippy::unwrap_used,
    reason = "test setup uses unwrap for infallible lang-id parse"
)]

fn loader() -> &'static i18n_embed::fluent::FluentLanguageLoader {
    static LOADER: std::sync::OnceLock<i18n_embed::fluent::FluentLanguageLoader> =
        std::sync::OnceLock::new();
    LOADER.get_or_init(|| {
        i18n_embed::fluent::FluentLanguageLoader::new("test", "en-US".parse().unwrap())
    })
}

mod i18n {
    use super::loader;
    ene_i18n_macros::i18n_keys!("tests/i18n/en-US/test.ftl", "tests/i18n/ja/test.ftl");
}

#[test]
fn no_arg_message_generates_fn_returning_string() {
    let result: String = i18n::hello();
    // The loader has no bundled translations, so fl! returns the key itself.
    assert!(!result.is_empty());
}

#[test]
fn single_param_message_generates_fn_with_arg() {
    let result: String = i18n::greeting("world");
    assert!(!result.is_empty());
}

#[test]
fn two_param_message_generates_fn_with_two_args() {
    let result: String = i18n::two_params("a", "b");
    assert!(!result.is_empty());
}

#[test]
fn keyword_key_becomes_raw_identifier() {
    // FTL key `type` is a Rust keyword → generated as `r#type`
    let result: String = i18n::r#type("foo");
    assert!(!result.is_empty());
}
