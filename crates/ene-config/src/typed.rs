use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_language() -> String {
    "ja".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_language")]
    pub language: String,

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

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("language must not be empty")]
    EmptyLanguage,
    #[error("data_dir must not be empty")]
    EmptyDataDir,
    #[error("configuration read failed: {0}")]
    Read(#[from] std::io::Error),
    #[error("configuration JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl Config {
    /// # Errors
    ///
    /// Returns [`ConfigError::EmptyLanguage`] when [`Config::language`] is
    /// empty or whitespace-only, or [`ConfigError::EmptyDataDir`] when an
    /// explicit [`Config::data_dir`] is the empty path.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.language.trim().is_empty() {
            return Err(ConfigError::EmptyLanguage);
        }
        if let Some(dir) = &self.data_dir
            && dir.as_os_str().is_empty()
        {
            return Err(ConfigError::EmptyDataDir);
        }
        Ok(())
    }

    /// Loads defaults, an optional JSON file, then ENE_LANGUAGE and ENE_DATA_DIR.
    /// Missing files and non-Unicode environment values are ignored.
    ///
    /// # Errors
    /// Returns a read or JSON error for an unreadable or malformed file, or
    /// EmptyLanguage when the final language is blank, or EmptyDataDir when
    /// the final data directory is the empty path.
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
