fn main() {
    // A path that does not exist must produce a clear compile error.
    ene_i18n_macros::i18n_keys!("/nonexistent/path/to/catalog.ftl");
}
