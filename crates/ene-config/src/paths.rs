use std::path::PathBuf;

use directories::ProjectDirs;

use crate::typed::Config;

/// Backed by the `"dev"` / `"ene"` / `"ene"` qualifier/organization/application
/// triple, which determines concrete paths such as `~/.local/share/ene` on
/// Linux.
pub fn default_data_dir() -> Option<PathBuf> {
    ProjectDirs::from("dev", "ene", "ene").map(|dirs| dirs.data_dir().to_path_buf())
}

pub fn resolve_data_dir(cfg: &Config) -> Option<PathBuf> {
    cfg.data_dir.clone().or_else(default_data_dir)
}
