//! Typed configuration values with layered loading and validation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_language() -> String {
    "ja".to_string()
}

/// Minimal `Stage 1` process configuration.
///
/// There are deliberately no secret-typed fields (no `secret`, `token`, or
/// `password` keys), no domain state, and no runtime judgments: autostart
/// selection belongs to a future presentation crate, not here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// UI locale name such as `"ja"`.
    ///
    /// Must not be empty or whitespace-only; see [`Config::validate`].
    #[serde(default = "default_language")]
    pub language: String,

    /// Explicit override; [`None`] (the default) selects the OS default from
    /// [`crate::paths::default_data_dir`].
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: default_language(),
            data_dir: None,
        }
    }
}

/// Failure to load or validate configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("language must not be empty")]
    EmptyLanguage,
    #[error("configuration read failed: {0}")]
    Read(#[from] std::io::Error),
    #[error("configuration JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl Config {
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

    /// Loads defaults, an optional JSON file, then ENE_LANGUAGE and ENE_DATA_DIR.
    /// Missing files and non-Unicode environment values are ignored.
    ///
    /// # Errors
    /// Returns a read or JSON error for an unreadable or malformed file, or
    /// EmptyLanguage when the final language is blank.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        load_with_env(path, |key| std::env::var(key).ok())
    }
}

fn load_with_env(
    path: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Config, ConfigError> {
    let mut config: Config = match path.map(std::fs::read).transpose() {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)?,
        Ok(None) => Config::default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(error) => return Err(error.into()),
    };
    if let Some(language) = env("ENE_LANGUAGE") {
        config.language = language;
    }
    if let Some(data_dir) = env("ENE_DATA_DIR") {
        config.data_dir = Some(PathBuf::from(data_dir));
    }
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_defaults_environment_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert_eq!(load_with_env(None, |_| None).unwrap(), Config::default());
        assert_eq!(
            load_with_env(Some(&path), |_| None).unwrap(),
            Config::default()
        );
        std::fs::write(&path, r#"{"data_dir":"from-file"}"#).unwrap();
        let file = load_with_env(Some(&path), |_| None).unwrap();
        assert_eq!(file.language, "ja");
        assert_eq!(file.data_dir, Some(PathBuf::from("from-file")));
        let overridden = load_with_env(Some(&path), |key| match key {
            "ENE_LANGUAGE" => Some("en".into()),
            "ENE_DATA_DIR" => Some("from-env".into()),
            _ => panic!("unexpected environment lookup"),
        })
        .unwrap();
        assert_eq!(overridden.language, "en");
        assert_eq!(overridden.data_dir, Some(PathBuf::from("from-env")));
        for blank in ["", "  ", "\t\n"] {
            assert!(matches!(
                load_with_env(Some(&path), |key| (key == "ENE_LANGUAGE")
                    .then(|| blank.into())),
                Err(ConfigError::EmptyLanguage)
            ));
        }
        std::fs::write(&path, r#"{"language":"fr"}"#).unwrap();
        assert_eq!(load_with_env(Some(&path), |_| None).unwrap().language, "fr");
    }

    #[test]
    fn invalid_files_are_reported_even_with_environment_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for invalid in ["{", r#"{"language":null}"#, r#"{"language":42}"#] {
            std::fs::write(&path, invalid).unwrap();
            assert!(matches!(
                load_with_env(Some(&path), |_| Some("en".into())),
                Err(ConfigError::Json(_))
            ));
        }
        assert!(matches!(
            load_with_env(Some(dir.path()), |_| None),
            Err(ConfigError::Read(_))
        ));
    }
}
