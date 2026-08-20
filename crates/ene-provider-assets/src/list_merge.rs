use ene_plugin_ipc::{AssetVersionView, AssetView, ListAssetsResult};

use crate::manifest::Manifest;
use crate::registry::host_catalog_plugin;
use crate::runtime_catalog::RuntimeCatalog;
use crate::store::resolve_active_binary;

#[must_use]
pub fn runtime_asset_hosted(plugin_id: &str, asset_id: &str) -> bool {
    host_catalog_plugin(plugin_id) && host_asset_ids(plugin_id).contains(&asset_id)
}

#[must_use]
pub fn host_asset_ids(plugin_id: &str) -> &'static [&'static str] {
    match plugin_id {
        "provider.gguf" => &["llama-server"],
        "provider.voicevox" => &["voicevox-engine"],
        _ => &[],
    }
}

/// Convert a fetched runtime catalog into IPC asset rows with install state.
#[must_use]
pub fn runtime_catalog_to_views(plugin_id: &str, catalog: &RuntimeCatalog) -> Vec<AssetView> {
    let manifest = Manifest::load(plugin_id);
    catalog
        .assets
        .iter()
        .map(|asset| {
            let local_path =
                resolve_active_binary(plugin_id, &asset.id).map(|path| path.display().to_string());
            let installed = local_path.is_some();
            let active_version = manifest.active_version(&asset.id).map(str::to_owned);
            let active = active_version.is_some() && installed;
            let versions = asset
                .releases
                .iter()
                .flat_map(|release| {
                    release.variants.iter().map(|variant| {
                        let install_key = RuntimeCatalog::install_key(&release.tag, &variant.id);
                        AssetVersionView {
                            version: install_key.clone(),
                            variant_id: variant.id.clone(),
                            label: variant.label.clone(),
                            backend: variant.backend.clone(),
                            release_tag: release.tag.clone(),
                            size_bytes: variant
                                .artifacts
                                .iter()
                                .filter_map(|row| row.size_bytes)
                                .reduce(u64::saturating_add),
                            recommended: variant.recommended,
                            installed: manifest
                                .active_version(&asset.id)
                                .is_some_and(|active| active == install_key),
                        }
                    })
                })
                .collect();
            AssetView {
                id: asset.id.clone(),
                kind: asset.kind.as_str().to_owned(),
                label: asset.label.clone(),
                description: asset.description.clone(),
                recommended: asset.recommended,
                installed,
                active,
                active_version,
                local_path,
                versions,
                seams: asset.seams.clone(),
            }
        })
        .collect()
}

/// Merge host runtime catalog assets with plugin-probed assets (weights).
#[must_use]
pub fn merge_host_catalog(
    plugin_id: &str,
    runtime: &RuntimeCatalog,
    probed: ListAssetsResult,
) -> ListAssetsResult {
    let mut host_assets = runtime_catalog_to_views(plugin_id, runtime);
    let hosted: std::collections::HashSet<&str> =
        host_asset_ids(plugin_id).iter().copied().collect();
    for asset in probed.assets {
        if !hosted.contains(asset.id.as_str()) {
            host_assets.push(asset);
        }
    }
    ListAssetsResult {
        assets: host_assets,
        error: probed.error.or_else(|| runtime.error.clone()),
    }
}
