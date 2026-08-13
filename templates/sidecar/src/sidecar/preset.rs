//! Engine preset / config-file generation.
//!
//! Sidecar engines that can serve multiple models from one process (e.g.
//! llama-server `--models-preset`) read a preset file from the work
//! directory. Implement `write_presets` for the engine's schema.

use std::path::Path;

use crate::config::SidecarConfig; // TODO: point at the plugin's config type.

/// Writes the engine preset file and returns its path.
pub fn write_presets(
    work_dir: &Path,
    _config: &SidecarConfig,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = work_dir.join("preset.json");
    // TODO: serialize the engine-specific preset from `config` profiles.
    Ok(path)
}
