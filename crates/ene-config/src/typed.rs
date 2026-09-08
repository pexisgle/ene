//! Typed configuration values with layered loading and validation.
//!
//! [`Config`] is the minimal `M1` configuration: a UI locale name and an
//! optional explicit data directory override. It carries no domain state, no
//! secret-typed fields, and no runtime judgments such as autostart selection.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Json, Serialized};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Returns the built-in default for [`Config::language`].
fn default_language() -> String {
    "ja".to_string()
}

/// Minimal `M1` process configuration.
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

/// Local configuration failure.
///
/// Defined with `std` only: this crate takes no `ene` dependencies and adds
/// no error-crate dependency for two variants. The `figment` failure is
/// boxed: `figment::Error` is over 200 bytes and must not bloat the enum or
/// every `Result` that carries it.
#[derive(Debug)]
pub enum ConfigError {
    /// [`Config::language`] was empty or whitespace-only.
    EmptyLanguage,
    /// Layered loading or extraction through `figment` failed.
    Figment(Box<figment::Error>),
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyLanguage => formatter.write_str("language must not be empty"),
            Self::Figment(error) => write!(formatter, "configuration load failed: {error}"),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EmptyLanguage => None,
            Self::Figment(error) => Some(&**error),
        }
    }
}

impl From<figment::Error> for ConfigError {
    /// Wraps a `figment` loading or extraction failure.
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
    /// malformed file is reported as [`ConfigError::Figment`]. The merged
    /// result is checked with [`Config::validate`] before it is returned, so
    /// an empty `ENE_LANGUAGE` fails the load.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Figment`] when a layer cannot be read or the
    /// merged values cannot be extracted, and
    /// [`ConfigError::EmptyLanguage`] when the merged [`Config::language`]
    /// is empty or whitespace-only.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut figment = Figment::from(Serialized::defaults(Self::default()));
        if let Some(path) = path {
            figment = figment.merge(Json::file(path));
        }
        let configured: Self = figment.merge(Env::prefixed("ENE_")).extract()?;
        configured.validate()?;
        Ok(configured)
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError};
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Serializes tests that read or mutate the process environment.
    ///
    /// [`Config::load`] always consults `ENE_*` variables and the process
    /// environment is global, so every test that calls `load` holds this
    /// lock and sanitizes the variables it reads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Acquires [`ENV_LOCK`], recovering from poisoning so one failing test
    /// cannot deadlock the rest.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Clears the `ENE_*` variables this crate reads and returns their
    /// previous values for later restoration with [`restore_env`].
    ///
    /// Must only be called while holding the guard returned by [`lock_env`].
    fn stash_env() -> (Option<String>, Option<String>) {
        let language = std::env::var("ENE_LANGUAGE").ok();
        let data_dir = std::env::var("ENE_DATA_DIR").ok();
        remove_env_var("ENE_LANGUAGE");
        remove_env_var("ENE_DATA_DIR");
        (language, data_dir)
    }

    /// Restores variables previously saved by [`stash_env`].
    ///
    /// Must only be called while holding the guard returned by [`lock_env`].
    fn restore_env(previous: (Option<String>, Option<String>)) {
        let (language, data_dir) = previous;
        match language {
            Some(value) => set_env_var("ENE_LANGUAGE", &value),
            None => remove_env_var("ENE_LANGUAGE"),
        }
        match data_dir {
            Some(value) => set_env_var("ENE_DATA_DIR", &value),
            None => remove_env_var("ENE_DATA_DIR"),
        }
    }

    /// Sets a process environment variable for these tests.
    ///
    /// Must only be called while holding the guard returned by [`lock_env`].
    fn set_env_var(key: &str, value: &str) {
        // SAFETY: every environment mutation in this test module goes through
        // this helper (or `remove_env_var` below) while holding `ENV_LOCK`,
        // and every environment read this crate performs (`stash_env` and
        // `Config::load`) happens under that same lock, so the mutation
        // cannot race with another environment access from this test binary.
        unsafe {
            std::env::set_var(key, value);
        }
    }

    /// Removes a process environment variable for these tests.
    ///
    /// Must only be called while holding the guard returned by [`lock_env`].
    fn remove_env_var(key: &str) {
        // SAFETY: same serialization argument as in `set_env_var`: all
        // mutation and all reads by this crate happen under `ENV_LOCK`.
        unsafe {
            std::env::remove_var(key);
        }
    }

    /// Writes `contents` to a fresh temporary file.
    ///
    /// Returns the file handle (which keeps the file alive) together with
    /// its path, or [`None`] when the file cannot be created or written.
    fn write_config_file(contents: &str) -> Option<(tempfile::NamedTempFile, PathBuf)> {
        let mut file: Option<tempfile::NamedTempFile> = tempfile::NamedTempFile::new().ok();
        if let Some(handle) = file.as_mut() {
            if handle.write_all(contents.as_bytes()).is_err() {
                return None;
            }
            if handle.flush().is_err() {
                return None;
            }
        }
        let file = file?;
        let path = file.path().to_path_buf();
        Some((file, path))
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
    fn load_without_file_or_env_returns_defaults() {
        let _guard = lock_env();
        let previous = stash_env();
        let loaded = Config::load(None);
        restore_env(previous);
        assert!(loaded.is_ok(), "load with no layers must succeed");
        let Some(config) = loaded.ok() else {
            return;
        };
        assert!(
            config == Config::default(),
            "load with no layers must return the built-in defaults"
        );
    }

    #[test]
    fn json_file_overrides_builtin_defaults() {
        let _guard = lock_env();
        let previous = stash_env();
        let written = write_config_file(r#"{"language": "de"}"#);
        let mut loaded: Option<Config> = None;
        if let Some((_, path)) = written.as_ref() {
            loaded = Config::load(Some(path.as_path())).ok();
        }
        restore_env(previous);
        assert!(written.is_some(), "temp config file must be created");
        assert!(loaded.is_some(), "load from a JSON file must succeed");
        let Some(config) = loaded else {
            return;
        };
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
        let _guard = lock_env();
        let previous = stash_env();
        let written = write_config_file(r#"{"language": "ja", "data_dir": "/tmp/ene-json-data"}"#);
        let mut loaded: Option<Config> = None;
        if let Some((_, path)) = written.as_ref() {
            loaded = Config::load(Some(path.as_path())).ok();
        }
        restore_env(previous);
        assert!(written.is_some(), "temp config file must be created");
        assert!(loaded.is_some(), "load from a JSON file must succeed");
        let Some(config) = loaded else {
            return;
        };
        assert!(
            config.data_dir == Some(PathBuf::from("/tmp/ene-json-data")),
            "JSON file must set the data_dir override"
        );
    }

    #[test]
    fn env_vars_take_precedence_over_the_json_file() {
        let _guard = lock_env();
        let previous = stash_env();
        let written = write_config_file(r#"{"language": "fr"}"#);
        set_env_var("ENE_LANGUAGE", "en");
        let mut loaded: Option<Config> = None;
        if let Some((_, path)) = written.as_ref() {
            loaded = Config::load(Some(path.as_path())).ok();
        }
        restore_env(previous);
        assert!(written.is_some(), "temp config file must be created");
        assert!(loaded.is_some(), "load with an env override must succeed");
        let Some(config) = loaded else {
            return;
        };
        assert!(
            config.language == "en",
            "ENE_LANGUAGE must win over the JSON file"
        );
    }

    #[test]
    fn env_data_dir_overrides_the_default() {
        let _guard = lock_env();
        let previous = stash_env();
        set_env_var("ENE_DATA_DIR", "/tmp/ene-env-data");
        let loaded = Config::load(None);
        restore_env(previous);
        assert!(
            loaded.is_ok(),
            "load with ENE_DATA_DIR must succeed: {loaded:?}"
        );
        let Some(config) = loaded.ok() else {
            return;
        };
        assert!(
            config.data_dir == Some(PathBuf::from("/tmp/ene-env-data")),
            "ENE_DATA_DIR must set the data_dir override"
        );
    }

    #[test]
    fn load_rejects_an_empty_language_from_the_env() {
        let _guard = lock_env();
        let previous = stash_env();
        set_env_var("ENE_LANGUAGE", "   ");
        let loaded = Config::load(None);
        restore_env(previous);
        assert!(
            matches!(loaded, Err(ConfigError::EmptyLanguage)),
            "a whitespace ENE_LANGUAGE must fail the load: {loaded:?}"
        );
    }

    #[test]
    fn config_roundtrips_through_json() {
        let original = Config {
            language: "en".to_string(),
            data_dir: Some(PathBuf::from("/tmp/ene-roundtrip")),
        };
        let json: Option<String> = serde_json::to_string(&original).ok();
        assert!(json.is_some(), "Config must serialize to JSON");
        let Some(json) = json else {
            return;
        };
        let back: Option<Config> = serde_json::from_str(&json).ok();
        assert!(
            back.is_some(),
            "Config must deserialize from its own JSON: {json}"
        );
        let Some(back) = back else {
            return;
        };
        assert!(original == back, "a serde roundtrip must preserve Config");
    }
}
