use std::path::{Path, PathBuf};

/// Plugin-owned asset store root.
#[must_use]
pub fn store_root(plugin_id: &str) -> PathBuf {
    ene_config::data_dir()
        .join("plugins")
        .join(plugin_id)
        .join("assets")
}

#[must_use]
pub fn manifest_path(plugin_id: &str) -> PathBuf {
    store_root(plugin_id).join("manifest.json")
}

#[must_use]
pub fn asset_path(plugin_id: &str, asset_id: &str, version: &str, filename: &str) -> PathBuf {
    store_root(plugin_id)
        .join(asset_id)
        .join(version)
        .join(filename)
}

#[must_use]
pub fn sidecar_binary_path(
    plugin_id: &str,
    asset_id: &str,
    version: &str,
    filename: &str,
) -> PathBuf {
    asset_path(plugin_id, asset_id, version, filename)
}

#[must_use]
pub fn weight_path(plugin_id: &str, asset_id: &str, filename: &str) -> PathBuf {
    store_root(plugin_id)
        .join("weights")
        .join(asset_id)
        .join(filename)
}

#[must_use]
pub fn resolve_installed_path(plugin_id: &str, asset_id: &str) -> Option<PathBuf> {
    let manifest = crate::manifest::Manifest::load(plugin_id);
    let record = manifest.installs.get(asset_id)?;
    let path = store_root(plugin_id).join(&record.relative_path);
    path.is_file().then_some(path)
}

#[must_use]
pub fn resolve_active_path(plugin_id: &str, asset_id: &str, filename: &str) -> Option<PathBuf> {
    let manifest = crate::manifest::Manifest::load(plugin_id);
    let version = manifest.active_version(asset_id)?;
    let path = asset_path(plugin_id, asset_id, version, filename);
    path.is_file().then_some(path)
}

pub(crate) fn ensure_parent(path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
