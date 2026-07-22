//! trybuild compile-fail tests for `i18n_keys!`.
//!
//! These verify that the macro produces clear compile errors for invalid
//! inputs. The pass case is covered by `tests/i18n_keys.rs` (a regular
//! integration test where `CARGO_MANIFEST_DIR` resolves correctly).

#[test]
fn i18n_keys_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/fail/*.rs");
}
