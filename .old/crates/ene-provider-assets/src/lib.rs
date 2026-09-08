//! Catalog, manifest, and verified downloads for provider plugins.

#![deny(unsafe_code)]
#![cfg_attr(test, expect(clippy::expect_used, reason = "tests fail fast"))]

mod allowlist;
mod catalog;
mod catalog_fetch;
mod download;
mod error;
mod list_merge;
mod manifest;
mod registry;
mod runtime_catalog;
mod store;

pub use allowlist::is_allowed_url;
pub use catalog::{
    AssetCatalog, AssetKind, CatalogAsset, CatalogVersion, PlatformTarget, current_platform,
    version_matches_platform,
};
pub use catalog_fetch::{
    CACHE_TTL, CachedCatalog, cache_stale, catalog_cache_path, fetch_runtime_catalog,
    load_cached_catalog, save_cached_catalog,
};
pub use download::{DownloadProgress, install_variant, install_version};
pub use error::AssetError;
pub use list_merge::{
    host_asset_ids, merge_host_catalog, runtime_asset_hosted, runtime_catalog_to_views,
};
pub use manifest::{InstallRecord, Manifest};
pub use registry::{CatalogRegistry, HOST_CATALOG_PLUGINS, host_catalog_plugin, shared_registry};
pub use runtime_catalog::{
    CatalogArtifact, CatalogRelease, CatalogVariant, RuntimeCatalog, RuntimeCatalogAsset,
    RuntimePlatform,
};
pub use store::{
    asset_path, resolve_active_binary, resolve_active_path, resolve_installed_path, store_root,
    variant_root, weight_path,
};
