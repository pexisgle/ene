//! Catalog, manifest, and verified downloads for provider plugins.

#![deny(unsafe_code)]

mod catalog;
mod download;
mod error;
mod manifest;
mod store;

pub use catalog::{
    AssetCatalog, AssetKind, CatalogAsset, CatalogVersion, PlatformTarget, current_platform,
    version_matches_platform,
};
pub use download::{DownloadProgress, install_version};
pub use error::AssetError;
pub use manifest::{InstallRecord, Manifest};
pub use store::{asset_path, resolve_active_path, resolve_installed_path, store_root, weight_path};
