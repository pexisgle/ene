use serde::{Deserialize, Serialize};

use crate::catalog::AssetKind;

/// OS + arch filter for a catalog variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePlatform {
    pub os: String,
    pub arch: String,
}

/// How to extract a downloaded artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractMode {
    RawFile,
    ZipMember { member: String },
    ZipTree,
}

/// One downloadable file in a variant (main binary zip, CUDA runtime pack, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogArtifact {
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub extract: ExtractMode,
    /// Relative path under the variant install directory (usually empty for tree).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dest: String,
}

/// One installable backend/build of a release (cpu-avx2, cuda-12.4, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogVariant {
    pub id: String,
    pub label: String,
    pub platform: RuntimePlatform,
    pub backend: String,
    #[serde(default)]
    pub recommended: bool,
    pub artifacts: Vec<CatalogArtifact>,
    /// Binary path relative to the variant install root (`run.exe`, `llama-server`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_binary: Option<String>,
}

/// One upstream release tag (llama `b10442`, VOICEVOX `0.25.2`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRelease {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    pub variants: Vec<CatalogVariant>,
}

/// One managed asset (`llama-server`, `voicevox-engine`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCatalogAsset {
    pub id: String,
    pub kind: AssetKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seams: Vec<String>,
    pub releases: Vec<CatalogRelease>,
}

/// Full plugin catalog fetched from GitHub and cached on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCatalog {
    pub plugin_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<RuntimeCatalogAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RuntimeCatalog {
    #[must_use]
    pub fn asset(&self, asset_id: &str) -> Option<&RuntimeCatalogAsset> {
        self.assets.iter().find(|row| row.id == asset_id)
    }

    #[must_use]
    pub fn find_variant<'a>(
        &'a self,
        asset_id: &str,
        release_tag: &str,
        variant_id: &str,
    ) -> Option<(
        &'a RuntimeCatalogAsset,
        &'a CatalogRelease,
        &'a CatalogVariant,
    )> {
        let asset = self.asset(asset_id)?;
        let release = asset.releases.iter().find(|row| row.tag == release_tag)?;
        let variant = release.variants.iter().find(|row| row.id == variant_id)?;
        Some((asset, release, variant))
    }

    /// Manifest / install key: `{tag}/{variant_id}`.
    #[must_use]
    pub fn install_key(release_tag: &str, variant_id: &str) -> String {
        format!("{release_tag}/{variant_id}")
    }

    #[must_use]
    pub fn split_install_key(key: &str) -> Option<(&str, &str)> {
        let (tag, variant) = key.split_once('/')?;
        if tag.is_empty() || variant.is_empty() {
            return None;
        }
        Some((tag, variant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_key_roundtrip() {
        let key = RuntimeCatalog::install_key("b4282", "cpu-avx2");
        assert_eq!(key, "b4282/cpu-avx2");
        assert_eq!(
            RuntimeCatalog::split_install_key(&key),
            Some(("b4282", "cpu-avx2"))
        );
    }
}
