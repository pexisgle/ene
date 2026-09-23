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
