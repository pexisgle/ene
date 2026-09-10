//! OS data directory resolution without side effects.
//!
//! The explicit [`Config`] override wins; otherwise the
//! OS default applies. Nothing here performs I/O or creates directories.

use std::path::PathBuf;

use directories::ProjectDirs;

use crate::typed::Config;

/// Backed by the `"dev"` / `"ene"` / `"ene"` qualifier/organization/application
/// triple. That choice is `Stage 1`-provisional and user-visible: it determines
/// concrete paths such as `~/.local/share/ene` on Linux, so any future change
/// must migrate existing directories instead of silently switching paths.
pub fn default_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "ene", "ene").map(|dirs| dirs.data_dir().to_path_buf())
}

/// Callers decide when (and whether) the resolved directory must exist.
pub fn resolve_data_dir(cfg: &Config) -> Option<PathBuf> {
    cfg.data_dir.clone().or_else(default_data_dir)
}

#[cfg(test)]
mod tests {
    use super::{default_data_dir, resolve_data_dir};
    use crate::typed::Config;
    use std::path::PathBuf;

    #[test]
    fn explicit_data_dir_wins_over_the_os_default() {
        let explicit = PathBuf::from("/tmp/ene-explicit-data");
        let config = Config {
            language: "ja".to_string(),
            data_dir: Some(explicit.clone()),
        };
        assert!(
            resolve_data_dir(&config) == Some(explicit),
            "an explicit data_dir must win over the OS default"
        );
    }

    #[test]
    fn missing_override_falls_back_to_the_os_default() {
        let config = Config {
            language: "ja".to_string(),
            data_dir: None,
        };
        assert!(
            resolve_data_dir(&config) == default_data_dir(),
            "a missing override must fall back to the OS default"
        );
    }

    #[test]
    fn os_default_is_absolute_when_present() {
        if let Some(dir) = default_data_dir() {
            assert!(
                dir.is_absolute(),
                "the OS default data dir must be absolute: {dir:?}"
            );
        }
    }

    #[test]
    fn resolution_creates_no_directories() {
        let probe =
            std::env::temp_dir().join(format!("ene-config-m1-probe-{}", std::process::id()));
        let config = Config {
            language: "ja".to_string(),
            data_dir: Some(probe.clone()),
        };
        let resolved = resolve_data_dir(&config);
        assert!(
            resolved == Some(probe),
            "resolution must echo the explicit override"
        );
        let created = match resolved.as_ref() {
            Some(path) => path.exists(),
            None => false,
        };
        assert!(
            !created,
            "resolution must not create directories as a side effect"
        );
    }
}
