use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::store::{ensure_parent, manifest_path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InstallRecord {
    pub relative_path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub active: BTreeMap<String, String>,
    #[serde(default)]
    pub installs: BTreeMap<String, InstallRecord>,
}

impl Manifest {
    #[must_use]
    pub fn load(plugin_id: &str) -> Self {
        let path = manifest_path(plugin_id);
        if !path.is_file() {
            return Self::default();
        }
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, plugin_id: &str) -> Result<(), std::io::Error> {
        let path = manifest_path(plugin_id);
        ensure_parent(&path)?;
        let raw = serde_json::to_string_pretty(self)?;
        atomic_write(&path, raw.as_bytes())
    }

    #[must_use]
    pub fn is_installed(&self, asset_id: &str) -> bool {
        self.installs.contains_key(asset_id)
    }

    #[must_use]
    pub fn active_version(&self, asset_id: &str) -> Option<&str> {
        self.active.get(asset_id).map(String::as_str)
    }

    pub fn set_active(&mut self, asset_id: impl Into<String>, version: impl Into<String>) {
        self.active.insert(asset_id.into(), version.into());
    }

    pub fn register_install(&mut self, asset_id: impl Into<String>, record: InstallRecord) {
        self.installs.insert(asset_id.into(), record);
    }
}

pub(crate) fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    ensure_parent(parent)?;
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let plugin_id = "provider.test";
        let root = tmp.path().join("plugins").join(plugin_id).join("assets");
        std::fs::create_dir_all(&root).expect("dir");
        let path = root.join("manifest.json");
        let mut manifest = Manifest::default();
        manifest.set_active("llama-server", "b1");
        manifest.register_install(
            "llama-server",
            InstallRecord {
                relative_path: "llama-server/b1/engine".into(),
                sha256: "abc".into(),
                version: Some("b1".into()),
            },
        );
        let raw = serde_json::to_string_pretty(&manifest).expect("json");
        atomic_write(&path, raw.as_bytes()).expect("write");
        let loaded: Manifest = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        assert_eq!(
            loaded.active.get("llama-server").map(String::as_str),
            Some("b1")
        );
    }
}
