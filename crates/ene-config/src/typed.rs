//! Typed configuration values with layered loading and validation.
//!
//! [`Config`] is the minimal `Stage 1` configuration: a UI locale name and an
//! optional explicit data directory override. It carries no domain state, no
//! secret-typed fields, and no runtime judgments such as autostart selection.

use std::path::{Path, PathBuf};

use std::ffi::OsString;

use figment::Figment;
use figment::providers::{Format, Json, Serialized};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prefix for environment-sourced overrides such as `ENE_LANGUAGE`.
const ENV_PREFIX: &str = "ENE_";

/// Suffix selecting [`Config::language`].
const LANGUAGE_SUFFIX: &str = "LANGUAGE";

/// Suffix selecting [`Config::data_dir`].
const DATA_DIR_SUFFIX: &str = "DATA_DIR";

/// Returns the built-in default for [`Config::language`].
fn default_language() -> String {
    "ja".to_string()
}

/// Minimal `Stage 1` process configuration.
///
/// Holds only general startup values owned by no domain: the UI locale name
/// and an optional explicit data directory override. There are deliberately
/// no secret-typed fields (no `secret`, `token`, or `password` keys), no
/// domain state, and no runtime judgments: autostart selection belongs to a
/// future presentation crate, not here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// UI locale name such as `"ja"`.
    ///
    /// Must not be empty or whitespace-only; see [`Config::validate`].
    #[serde(default = "default_language")]
    pub language: String,

    /// Explicit data directory override.
    ///
    /// [`None`] (the default) selects the OS default from
    /// [`crate::paths::default_data_dir`].
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
}

impl Default for Config {
    /// Returns the built-in defaults: Japanese locale, no directory override.
    fn default() -> Self {
        Self {
            language: default_language(),
            data_dir: None,
        }
    }
}

/// Local configuration failure, derived with `thiserror` per the repository
/// convention for library errors.
///
/// The `figment` failure is boxed: `figment::Error` is over 200 bytes and
/// must not bloat the enum or every `Result` that carries it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// [`Config::language`] was empty or whitespace-only.
    #[error("language must not be empty")]
    EmptyLanguage,
    /// Layered loading or extraction through `figment` failed.
    #[error("configuration load failed: {0}")]
    Figment(#[from] Box<figment::Error>),
}

impl From<figment::Error> for ConfigError {
    /// Boxes a `figment` loading or extraction failure.
    fn from(error: figment::Error) -> Self {
        Self::Figment(Box::new(error))
    }
}

impl Config {
    /// Checks the configuration values without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::EmptyLanguage`] when [`Config::language`] is
    /// empty or whitespace-only.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.language.trim().is_empty() {
            Err(ConfigError::EmptyLanguage)
        } else {
            Ok(())
        }
    }

    /// Loads configuration from layered sources of increasing precedence:
    /// built-in defaults, then the JSON file at `path` when [`Some`], then
    /// environment variables prefixed with `ENE_` (for example,
    /// `ENE_LANGUAGE` or `ENE_DATA_DIR`).
    ///
    /// A missing file contributes no values; a present but unreadable or
    /// malformed file is reported as [`ConfigError::Figment`]. Environment
    /// selection and application are pure ([`select_env`], [`apply_env`]) so
    /// precedence is unit-testable without mutating the process environment;
    /// only this entry point reads the real one, and non-UTF-8 entries are
    /// ignored rather than read at all. The merged result is checked with
    /// [`Config::validate`] before it is returned, so an empty
    /// `ENE_LANGUAGE` fails the load.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Figment`] when a layer cannot be read or the
    /// merged values cannot be extracted, and
    /// [`ConfigError::EmptyLanguage`] when the merged [`Config::language`]
    /// is empty or whitespace-only.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let base = file_layers(path)?;
        let selected = select_env(Self::native_env_pairs());
        let merged = apply_env(base, selected);
        merged.validate()?;
        Ok(merged)
    }

    /// Reads the process environment without claiming UTF-8 handling.
    ///
    /// Entries that are not valid Unicode on either side are dropped here,
    /// before selection, so a stray non-UTF-8 variable can neither panic the
    /// load nor leak undecodable bytes into configuration values.
    fn native_env_pairs() -> Vec<(OsString, OsString)> {
        std::env::vars_os().collect()
    }
}

/// Loads the file-backed layers (built-in defaults, then the JSON file at
/// `path` when [`Some`]) without consulting the environment and without
/// validating.
///
/// A missing file contributes no values; a present but unreadable or
/// malformed file is reported as [`ConfigError::Figment`]. Callers apply
/// [`select_env`]/[`apply_env`] and then [`Config::validate`].
///
/// # Errors
///
/// Returns [`ConfigError::Figment`] when the file cannot be read or the
/// merged values cannot be extracted.
fn file_layers(path: Option<&Path>) -> Result<Config, ConfigError> {
    let mut figment = Figment::from(Serialized::defaults(Config::default()));
    if let Some(path) = path {
        figment = figment.merge(Json::file(path));
    }
    Ok(figment.extract()?)
}

/// Selects this crate's overrides from environment-style pairs.
///
/// Only keys starting with `ENE_` are kept, with the prefix stripped, so
/// `ENE_LANGUAGE` becomes `("LANGUAGE", value)`. Unknown suffixes are kept
/// as well; [`apply_env`] decides which ones take effect. Selection is a
/// pure function of its input, which keeps precedence testable without
/// touching the process environment.
fn select_env(pairs: Vec<(OsString, OsString)>) -> Vec<(String, String)> {
    let mut selected = Vec::new();
    for (key, value) in pairs {
        let (Some(key), Some(value)) = (key.into_string().ok(), value.into_string().ok()) else {
            continue;
        };
        if let Some(suffix) = key.strip_prefix(ENV_PREFIX) {
            selected.push((suffix.to_owned(), value));
        }
    }
    selected
}

/// Applies previously selected environment overrides to `base`.
///
/// `LANGUAGE` replaces the locale, `DATA_DIR` replaces the data directory
/// override, and any other suffix is ignored. Application is a pure function
/// of its inputs; validation stays with the caller.
fn apply_env(mut base: Config, overrides: Vec<(String, String)>) -> Config {
    for (suffix, value) in overrides {
        if suffix == LANGUAGE_SUFFIX {
            base.language = value;
        } else if suffix == DATA_DIR_SUFFIX {
            base.data_dir = Some(PathBuf::from(value));
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError, apply_env, file_layers, select_env};
    use std::ffi::OsString;
    use std::io::Write as _;
    use std::path::PathBuf;

    /// Builds native-style pairs from plain strings for [`select_env`].
    fn native(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    /// Writes `contents` to a temp file. Keep the handle; drop deletes it.
    fn write_config_file(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("temp config file must be created");
        file.write_all(contents.as_bytes())
            .expect("temp config file must be writable");
        file.flush().expect("temp config file must flush");
        file
    }

    #[test]
    fn defaults_use_japanese_with_no_data_dir_override() {
        let config = Config::default();
        assert!(config.language == "ja", "default language must be Japanese");
        assert!(
            config.data_dir.is_none(),
            "default data_dir must be no override"
        );
    }

    #[test]
    fn validate_accepts_a_non_empty_language() {
        let config = Config {
            language: "ja".to_string(),
            data_dir: None,
        };
        assert!(
            config.validate().is_ok(),
            "a non-empty language must validate"
        );
    }

    #[test]
    fn validate_rejects_empty_and_whitespace_language() {
        for language in ["", "   ", "\t\n  "] {
            let config = Config {
                language: language.to_string(),
                data_dir: None,
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::EmptyLanguage)),
                "language {language:?} must fail validation"
            );
        }
    }

    #[test]
    fn file_layers_without_a_file_return_defaults() {
        let config = file_layers(None).expect("no layers must succeed");
        assert!(
            config == Config::default(),
            "no layers must return the built-in defaults"
        );
    }

    #[test]
    fn json_file_overrides_builtin_defaults() {
        let file = write_config_file(r#"{"language": "de"}"#);
        let config = file_layers(Some(file.path())).expect("load from a JSON file must succeed");
        assert!(
            config.language == "de",
            "JSON file must override the default language"
        );
        assert!(
            config.data_dir.is_none(),
            "an unspecified data_dir must stay no override"
        );
    }

    #[test]
    fn json_file_can_set_data_dir() {
        let file = write_config_file(r#"{"language": "ja", "data_dir": "/tmp/ene-json-data"}"#);
        let config = file_layers(Some(file.path())).expect("load from a JSON file must succeed");
        assert!(
            config.data_dir == Some(PathBuf::from("/tmp/ene-json-data")),
            "JSON file must set the data_dir override"
        );
    }

    #[test]
    fn malformed_json_file_is_reported_not_absorbed() {
        let file = write_config_file(r#"{"language": "#);
        assert!(
            matches!(file_layers(Some(file.path())), Err(ConfigError::Figment(_))),
            "a malformed JSON file must fail the load"
        );
    }

    #[test]
    fn selection_strips_the_prefix_and_ignores_other_variables() {
        let selected = select_env(native(&[
            ("ENE_LANGUAGE", "en"),
            ("ENE_DATA_DIR", "/tmp/ene-env-data"),
            ("ENE_UNRECOGNISED", "kept for apply_env to ignore"),
            ("UNRELATED", "dropped"),
            ("ENE_", "empty suffix, kept for apply_env to ignore"),
        ]));
        assert!(
            selected.contains(&("LANGUAGE".to_string(), "en".to_string())),
            "ENE_LANGUAGE must be selected: {selected:?}"
        );
        assert!(
            selected.contains(&("DATA_DIR".to_string(), "/tmp/ene-env-data".to_string())),
            "ENE_DATA_DIR must be selected: {selected:?}"
        );
        assert!(
            !selected.iter().any(|(suffix, _)| suffix == "UNRELATED"),
            "unprefixed variables must be dropped: {selected:?}"
        );
    }

    #[test]
    fn env_overrides_win_over_file_values() {
        let base = Config {
            language: "fr".to_string(),
            data_dir: None,
        };
        let merged = apply_env(base, vec![("LANGUAGE".to_string(), "en".to_string())]);
        assert!(
            merged.language == "en",
            "an env language must win over the file value"
        );
        assert!(
            merged.data_dir.is_none(),
            "an untouched data_dir must stay no override"
        );
    }

    #[test]
    fn env_data_dir_overrides_the_default() {
        let merged = apply_env(
            Config::default(),
            vec![("DATA_DIR".to_string(), "/tmp/ene-env-data".to_string())],
        );
        assert!(
            merged.data_dir == Some(PathBuf::from("/tmp/ene-env-data")),
            "an env data_dir must set the override"
        );
    }

    #[test]
    fn unknown_suffixes_are_ignored() {
        let base = Config::default();
        let merged = apply_env(
            base.clone(),
            vec![("UNRECOGNISED".to_string(), "x".to_string())],
        );
        assert!(
            merged == base,
            "unknown env suffixes must not change the configuration"
        );
    }

    #[test]
    fn file_then_env_compose_like_load() {
        let file = write_config_file(r#"{"language": "fr"}"#);
        let base = file_layers(Some(file.path())).expect("file layers must load");
        let selected = select_env(native(&[("ENE_LANGUAGE", "en")]));
        let merged = apply_env(base, selected);
        assert!(
            merged.language == "en",
            "an env language must win over the file value"
        );
        assert!(
            merged.validate().is_ok(),
            "the composed configuration must validate"
        );
    }

    #[test]
    fn empty_env_language_fails_validation() {
        let merged = apply_env(
            Config::default(),
            vec![("LANGUAGE".to_string(), "   ".to_string())],
        );
        assert!(
            matches!(merged.validate(), Err(ConfigError::EmptyLanguage)),
            "a whitespace env language must fail validation"
        );
    }

    #[test]
    fn config_roundtrips_through_json() {
        let original = Config {
            language: "en".to_string(),
            data_dir: Some(PathBuf::from("/tmp/ene-roundtrip")),
        };
        let json = serde_json::to_string(&original).expect("Config must serialize to JSON");
        let back: Config =
            serde_json::from_str(&json).expect("Config must deserialize from its own JSON");
        assert!(original == back, "a serde roundtrip must preserve Config");
    }
}
