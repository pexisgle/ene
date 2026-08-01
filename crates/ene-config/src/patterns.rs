//! Deterministic pattern packs with multi-language support.
//!
//! Forget-detection regexes are loaded at runtime from
//! `assets/lang/{lang}/patterns.json` (see [`crate::paths::pattern_pack_path`]),
//! keeping the pattern corpus out of compiled code. Adding a language is a
//! matter of dropping a new `assets/lang/{lang}/patterns.json` pack — no Rust
//! change to the loading path is required. Listing the code in
//! [`crate::prompts::SUPPORTED_LANGUAGES`] additionally embeds a compile-time fallback so the
//! language still works when the asset is missing.
//!
//! When a runtime pack is missing or unreadable (e.g. during unit tests or CI,
//! where no assets directory is deployed), [`PatternLibrary::load`] falls back
//! to the compile-time embedded pack for that language. Only the languages in
//! [`crate::prompts::SUPPORTED_LANGUAGES`] carry an embedded fallback; an unsupported language
//! code falls back to English.
//!
//! # Usage
//!
//! ```rust,no_run
//! use ene_config::PatternLibrary;
//!
//! let lib = PatternLibrary::load("en");
//! for pattern in lib.forget_patterns() {
//!     println!("{}: {}", pattern.name, pattern.regex);
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::prompts::{is_embedded_language, resolve_language_alias};

/// A single forget-detection pattern.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ForgetPattern {
    /// Human-readable name for diagnostics (e.g. `"object_before_keyword"`).
    pub name: String,
    /// Rust `regex`-syntax expression matching a forget request in this
    /// language. The capture group at [`Self::target_group`] holds the
    /// deletion target.
    pub regex: String,
    /// 1-based capture-group index within `regex` holding the deletion target.
    pub target_group: usize,
    /// Whether the English question-start heuristic (a `?` or an interrogative
    /// opener such as "do you") should suppress a match. Prevents phrasing
    /// like "did you forget my name?" from being captured as an instruction.
    /// The heuristic is English-oriented; packs for other languages should set
    /// this to `false` unless the phrasing applies.
    #[serde(default)]
    pub skip_questions: bool,
}

/// Strongly typed pattern-pack layout mapping to `assets/lang/{lang}/patterns.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatternPackData {
    /// Forget-detection patterns, tried in pack order; every matching pattern
    /// yields a candidate (duplicates are collapsed by the caller).
    pub forget_patterns: Vec<ForgetPattern>,
}

/// Loads and accesses deterministic patterns from a JSON locale file.
#[derive(Debug, Clone)]
pub struct PatternLibrary {
    data: PatternPackData,
    lang: String,
}

/// Builds a [`PatternLibrary`] from the compile-time embedded pack for `$lang`.
///
/// The embedded pack is the shipped `assets/lang/{lang}/patterns.json` itself
/// (embedded via `include_str!`), so the fallback and the runtime asset stay
/// byte-for-byte identical by construction.
macro_rules! embedded_pattern_pack {
    ($lang:literal) => {{
        let data: PatternPackData = parse_embedded_patterns(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/lang/",
            $lang,
            "/patterns.json"
        )));
        PatternLibrary {
            data,
            lang: $lang.to_string(),
        }
    }};
}

/// Parses the bundled pattern metadata JSON embedded via `include_str!`.
///
/// Kept as a regular function (rather than inlined in the
/// `embedded_pattern_pack!` macro) so the `#[expect]` below is attached to a
/// normal item — `#[expect]` does not fulfil correctly when expanded from
/// `macro_rules!`.
#[expect(
    clippy::expect_used,
    reason = "bundled JSON is validated at build time; parse failure is a release-blocker"
)]
fn parse_embedded_patterns(json: &str) -> PatternPackData {
    serde_json::from_str(json).expect("bundled patterns.json is a release-blocker if invalid")
}

impl PatternLibrary {
    /// Loads the pattern library for the given language code (e.g. `"en"`).
    ///
    /// The runtime pack at `assets/lang/{lang}/patterns.json` is preferred so
    /// patterns can be tuned without recompiling. When that pack is missing or
    /// unreadable, the compile-time embedded pack for the language is used
    /// instead (see [`crate::prompts::SUPPORTED_LANGUAGES`]). Languages are matched
    /// case-insensitively by primary subtag (the legacy alias `"jp"` maps to
    /// `"ja"`); a language with neither a runtime pack nor an embedded
    /// fallback falls back to English.
    pub fn load(lang: &str) -> Self {
        Self::load_from(crate::paths::assets_dir(), lang)
    }

    /// Loads the pattern library for `lang`, resolving the runtime pack
    /// against an explicit base assets directory (`base/lang/{code}/patterns.json`).
    ///
    /// This is the testable core of [`PatternLibrary::load`]; production
    /// callers use `load`, which passes the process-wide
    /// [`crate::paths::assets_dir`].
    fn load_from(base: &std::path::Path, lang: &str) -> Self {
        let code = resolve_language_alias(lang);
        match Self::load_from_assets(base, &code) {
            Ok(lib) => lib,
            Err(err) => {
                if is_embedded_language(&code) {
                    tracing::debug!(
                        language = %code,
                        error = %err,
                        "runtime pattern pack unavailable; using embedded fallback"
                    );
                    Self::built_in(&code)
                } else {
                    tracing::warn!(
                        language = %code,
                        error = %err,
                        "no runtime pattern pack and no embedded fallback; using English"
                    );
                    Self::built_in_english()
                }
            }
        }
    }

    /// Reads and parses the runtime pattern pack for `code` from
    /// `base/lang/{code}/patterns.json`.
    fn load_from_assets(base: &std::path::Path, code: &str) -> Result<Self, crate::EneConfigError> {
        let path = crate::paths::pattern_pack_path_in(base, code);
        let contents = std::fs::read_to_string(&path).map_err(|source| {
            crate::EneConfigError::PatternPackRead {
                path: path.display().to_string(),
                source,
            }
        })?;
        let data: PatternPackData = serde_json::from_str(&contents)?;
        Ok(Self {
            data,
            lang: code.to_string(),
        })
    }

    /// Returns the compile-time embedded pack for a language code.
    ///
    /// These are the same patterns shipped under `assets/lang/{code}/` but
    /// embedded at compile time as a fallback so the application works even
    /// when assets are missing (e.g. during unit tests or CI). Embedded packs
    /// are necessarily static, so only [`crate::prompts::SUPPORTED_LANGUAGES`] are available
    /// here; any other code falls back to English.
    pub fn built_in(code: &str) -> Self {
        match resolve_language_alias(code).as_str() {
            "ja" => Self::built_in_japanese(),
            _ => Self::built_in_english(),
        }
    }

    /// Returns the built-in compile-time English defaults.
    ///
    /// These are the same patterns shipped in `assets/lang/en/patterns.json`
    /// but embedded at compile time as a fallback so the application works
    /// even when assets are missing (e.g. during unit tests or CI).
    pub fn built_in_english() -> Self {
        embedded_pattern_pack!("en")
    }

    /// Returns the built-in compile-time Japanese defaults.
    ///
    /// These are the same patterns shipped in `assets/lang/ja/patterns.json`
    /// but embedded at compile time as a fallback.
    pub fn built_in_japanese() -> Self {
        embedded_pattern_pack!("ja")
    }

    /// Language code (e.g. `"en"` or `"ja"`) for this pattern library instance.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Forget-detection patterns for this language, in match order.
    pub fn forget_patterns(&self) -> &[ForgetPattern] {
        &self.data.forget_patterns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::SUPPORTED_LANGUAGES;

    /// Writes the embedded pack for `lang` to `base/lang/{lang}/patterns.json`,
    /// mirroring the on-disk layout the runtime loader expects.
    fn write_pack(base: &std::path::Path, lang: &str) {
        let lib = PatternLibrary::built_in(lang);
        let dir = base.join("lang").join(lang);
        std::fs::create_dir_all(&dir).expect("create pack dir");
        let json = serde_json::to_string_pretty(&lib.data).expect("serialize embedded pack");
        std::fs::write(dir.join("patterns.json"), json).expect("write pack");
    }

    #[test]
    fn embedded_packs_parse_and_are_complete() {
        // Language-pack validation (#354): the embedded packs for every
        // supported language must parse and contain forget patterns.
        for lang in SUPPORTED_LANGUAGES {
            let lib = PatternLibrary::built_in(lang);
            assert_eq!(lib.lang(), *lang);
            assert!(
                !lib.forget_patterns().is_empty(),
                "{lang} pack must contain forget patterns"
            );
            for pattern in lib.forget_patterns() {
                assert!(!pattern.name.is_empty());
                assert!(!pattern.regex.is_empty());
                assert!(pattern.target_group > 0, "target_group is 1-based");
            }
        }
    }

    #[test]
    fn shipped_pack_files_parse() {
        // The shipped runtime packs under assets/lang/ are the same files the
        // embedded fallback is compiled from; both must parse and be complete.
        for lang in SUPPORTED_LANGUAGES {
            let asset_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/lang")
                .join(lang)
                .join("patterns.json");
            let asset_json = std::fs::read_to_string(&asset_path)
                .expect("shipped runtime pack must exist and be readable");
            let asset: PatternPackData =
                serde_json::from_str(&asset_json).expect("parse asset pack");
            assert!(
                !asset.forget_patterns.is_empty(),
                "{lang} asset pack must contain forget patterns"
            );
            assert_eq!(
                serde_json::to_value(&asset).expect("serialize asset pack"),
                serde_json::to_value(&PatternLibrary::built_in(lang).data)
                    .expect("serialize embedded pack"),
                "assets/lang/{lang}/patterns.json diverged from the embedded fallback"
            );
        }
    }

    #[test]
    fn load_prefers_runtime_asset_over_embedded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_pack(tmp.path(), "en");

        // Mutate the on-disk pack so it is distinguishable from the embedded copy.
        let pack_path = crate::paths::pattern_pack_path_in(tmp.path(), "en");
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&pack_path).expect("read pack"))
                .expect("parse pack");
        value["forget_patterns"][0]["name"] =
            serde_json::Value::String("runtime override".to_string());
        std::fs::write(
            &pack_path,
            serde_json::to_string_pretty(&value).expect("serialize mutated pack"),
        )
        .expect("write mutated pack");

        let lib = PatternLibrary::load_from(tmp.path(), "en");
        assert_eq!(lib.lang(), "en");
        assert_eq!(lib.forget_patterns()[0].name, "runtime override");
    }

    #[test]
    fn load_falls_back_to_embedded_when_asset_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No pack written: the loader must fall back to the embedded Japanese pack.
        let lib = PatternLibrary::load_from(tmp.path(), "ja");
        assert_eq!(lib.lang(), "ja");
        assert_eq!(
            serde_json::to_value(lib.forget_patterns()).expect("serialize"),
            serde_json::to_value(PatternLibrary::built_in_japanese().forget_patterns())
                .expect("serialize")
        );
    }

    #[test]
    fn load_falls_back_to_english_for_unknown_language() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // "fr" has neither a runtime pack nor an embedded fallback.
        let lib = PatternLibrary::load_from(tmp.path(), "fr");
        assert_eq!(lib.lang(), "en");
        assert_eq!(
            serde_json::to_value(lib.forget_patterns()).expect("serialize"),
            serde_json::to_value(PatternLibrary::built_in_english().forget_patterns())
                .expect("serialize")
        );
    }

    #[test]
    fn load_normalizes_language_aliases() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_pack(tmp.path(), "ja");

        for alias in ["ja", "JA", "ja-JP", "jp"] {
            let lib = PatternLibrary::load_from(tmp.path(), alias);
            assert_eq!(lib.lang(), "ja", "alias {alias} should resolve to ja");
        }
    }
}
