fn main() {
    // The English catalog has a key missing from the Japanese catalog,
    // which must produce a catalog-consistency compile error.
    // Absolute paths are used so the fixtures resolve regardless of the
    // trybuild temp crate directory.
    ene_i18n_macros::i18n_keys!(
        "/home/pexisgle/dev/Ene/crates/ene-i18n-macros/tests/trybuild/fixtures/en.ftl",
        "/home/pexisgle/dev/Ene/crates/ene-i18n-macros/tests/trybuild/fixtures/ja.ftl"
    );
}
