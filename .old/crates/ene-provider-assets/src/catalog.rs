use serde::{Deserialize, Serialize};

/// Asset kind string on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Sidecar,
    Weight,
}

impl AssetKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sidecar => "sidecar",
            Self::Weight => "weight",
        }
    }
}

/// Platform filter for a catalog version row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformTarget {
    pub os: &'static str,
    pub arch: &'static str,
}

#[must_use]
pub fn current_platform() -> PlatformTarget {
    PlatformTarget {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// One downloadable version in a catalog.
#[derive(Debug, Clone)]
pub struct CatalogVersion {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: Option<u64>,
    pub filename: &'static str,
    pub platform: Option<PlatformTarget>,
    pub recommended: bool,
    /// When set, the downloaded file is a zip containing `archive_member`.
    pub archive_member: Option<&'static str>,
}

/// Static catalog row defined by a provider plugin.
#[derive(Debug, Clone)]
pub struct CatalogAsset {
    pub id: &'static str,
    pub kind: AssetKind,
    pub label: &'static str,
    pub description: &'static str,
    pub recommended: bool,
    pub seams: &'static [&'static str],
    pub versions: &'static [CatalogVersion],
}

/// Lookup helpers over a static catalog table.
pub struct AssetCatalog {
    rows: &'static [CatalogAsset],
}

impl AssetCatalog {
    #[must_use]
    pub const fn new(rows: &'static [CatalogAsset]) -> Self {
        Self { rows }
    }

    #[must_use]
    pub fn all(&self) -> &'static [CatalogAsset] {
        self.rows
    }

    #[must_use]
    pub fn get(&self, asset_id: &str) -> Option<&'static CatalogAsset> {
        self.rows.iter().find(|row| row.id == asset_id)
    }

    #[must_use]
    pub fn version<'a>(
        &'a self,
        asset_id: &str,
        version: &str,
    ) -> Option<(&'a CatalogAsset, &'a CatalogVersion)> {
        let asset = self.get(asset_id)?;
        let ver = asset.versions.iter().find(|row| row.version == version)?;
        Some((asset, ver))
    }

    #[must_use]
    pub fn best_version<'a>(&'a self, asset: &'a CatalogAsset) -> Option<&'a CatalogVersion> {
        let platform = current_platform();
        asset
            .versions
            .iter()
            .filter(|row| version_matches_platform(row, platform))
            .find(|row| row.recommended)
            .or_else(|| {
                asset
                    .versions
                    .iter()
                    .find(|row| version_matches_platform(row, platform))
            })
    }

    #[must_use]
    pub fn is_allowlisted_url(&self, url: &str) -> bool {
        url.starts_with("https://")
            && self
                .rows
                .iter()
                .flat_map(|asset| asset.versions.iter())
                .any(|version| version.url == url)
    }
}

#[must_use]
pub fn version_matches_platform(version: &CatalogVersion, platform: PlatformTarget) -> bool {
    match version.platform {
        None => true,
        Some(target) => target.os == platform.os && target.arch == platform.arch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ROWS: &[CatalogAsset] = &[CatalogAsset {
        id: "gemma",
        kind: AssetKind::Weight,
        label: "Gemma",
        description: "",
        recommended: true,
        seams: &["seam.llm"],
        versions: &[CatalogVersion {
            version: "v1",
            url: "https://example.invalid/gemma.gguf",
            sha256: "00",
            size_bytes: None,
            filename: "gemma.gguf",
            platform: None,
            recommended: true,
            archive_member: None,
        }],
    }];

    #[test]
    fn allowlist_matches_catalog_urls() {
        let catalog = AssetCatalog::new(TEST_ROWS);
        assert!(catalog.is_allowlisted_url("https://example.invalid/gemma.gguf"));
        assert!(!catalog.is_allowlisted_url("https://evil.invalid/x"));
    }
}
