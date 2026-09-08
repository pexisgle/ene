//! Pack the shipped Alicia VRM into an `.enechar` zip for `POST /characters/import`.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(String),
    #[error("missing bundled VRM at {0}")]
    Missing(String),
}

#[cfg(test)]
pub(crate) const BUNDLED_ALICIA_B_ID: &str = "char.alicia-b";
#[cfg(test)]
pub(crate) const BUNDLED_ALICIA_B_NAME: &str = "Alicia B";

/// Build an ene character archive from the repo-shipped Alicia body assets.
pub fn pack_bundled_alicia() -> Result<Vec<u8>, BundleError> {
    pack_bundled_named("char.alicia", "Alicia")
}

/// Same Alicia mesh under a second package id so two occupants can render.
pub fn pack_bundled_named(id: &str, display_name: &str) -> Result<Vec<u8>, BundleError> {
    let root = ene_config::paths::assets_dir().join("characters/Alicia");
    pack_named_from(&root, id, display_name)
}

pub fn pack_alicia_from(root: &Path) -> Result<Vec<u8>, BundleError> {
    pack_named_from(root, "char.alicia", "Alicia")
}

pub fn pack_named_from(root: &Path, id: &str, display_name: &str) -> Result<Vec<u8>, BundleError> {
    let vrm = root.join("AliciaSolid.vrm");
    if !vrm.is_file() {
        return Err(BundleError::Missing(vrm.display().to_string()));
    }
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.toml".to_owned(),
        format!(
            r#"[package]
kind = "character"
id = "{id}"
version = "1.0.0"
format_version = 1
display_name = "{display_name}"

[contents]
soul = "embedded"
body = "embedded"

[integrity]
digest = ""
"#
        )
        .into_bytes(),
    );
    files.insert(
        "soul/soul.toml".to_owned(),
        format!(
            r#"[identity]
name = "{display_name}"
role = "companion"
locale_default = "en-US"

[persona]
source = "persona.md"
"#
        )
        .into_bytes(),
    );
    files.insert(
        "soul/persona.md".to_owned(),
        b"You are Alicia, a desktop companion. Keep responses relatively short and sweet, suitable for displaying on a screen overlay.\n"
            .to_vec(),
    );
    let card_path = root.join("character.json");
    if let Ok(card) = std::fs::read(card_path) {
        files.insert("soul/character.json".to_owned(), card);
    }
    files.insert(
        "body/body.toml".to_owned(),
        br#"[body]
kind = "vrm"
avatar = "avatar/model.vrm"

[expressions]
available = ["happy", "calm", "sad", "angry", "surprised", "blink"]
"#
        .to_vec(),
    );
    files.insert(
        "body/emotion_map.toml".to_owned(),
        br#"[map.happy]
expression = "happy"
intensity_scale = 1.0

[map.calm]
expression = "calm"
intensity_scale = 1.0
"#
        .to_vec(),
    );
    files.insert("body/avatar/model.vrm".to_owned(), std::fs::read(&vrm)?);
    let motions = root.join("motions");
    if motions.is_dir()
        && let Ok(entries) = std::fs::read_dir(&motions)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_vrma = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vrma"));
            if is_vrma && let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                files.insert(format!("body/motions/{name}"), std::fs::read(&path)?);
            }
        }
    }
    pack_zip(&files)
}

fn pack_zip(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, BundleError> {
    let mut buf = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            writer
                .start_file(name, options)
                .map_err(|err| BundleError::Zip(err.to_string()))?;
            writer
                .write_all(bytes)
                .map_err(|err| BundleError::Zip(err.to_string()))?;
        }
        writer
            .finish()
            .map_err(|err| BundleError::Zip(err.to_string()))?;
    }
    Ok(buf)
}

#[must_use]
pub fn motions_dir_for_package(package_path: &str) -> PathBuf {
    PathBuf::from(package_path).join("body/motions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn pack_alicia_from_missing_dir_errors() {
        let err = pack_alicia_from(Path::new("/no/such/alicia")).unwrap_err();
        assert!(matches!(err, BundleError::Missing(_)));
    }

    #[test]
    fn pack_zip_has_local_file_magic() {
        let mut files = BTreeMap::new();
        files.insert("manifest.toml".into(), b"ok".to_vec());
        let zip = pack_zip(&files).expect("zip");
        assert_eq!(&zip[..4], b"PK\x03\x04");
    }

    #[test]
    fn pack_alicia_from_embeds_vrm_and_motions() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("AliciaSolid.vrm"), b"vrm-bytes").unwrap();
        std::fs::create_dir_all(dir.path().join("motions")).unwrap();
        std::fs::write(dir.path().join("motions/VRMA_01.vrma"), b"vrma-bytes").unwrap();
        let bytes = pack_alicia_from(dir.path()).expect("pack");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(zip.by_name("manifest.toml").is_ok());
        assert!(zip.by_name("soul/soul.toml").is_ok());
        let mut persona = String::new();
        zip.by_name("soul/persona.md")
            .unwrap()
            .read_to_string(&mut persona)
            .unwrap();
        assert!(persona.contains("You are Alicia, a desktop companion"));
        assert!(!persona.contains("Ene"));
        assert!(zip.by_name("body/body.toml").is_ok());
        assert!(zip.by_name("body/avatar/model.vrm").is_ok());
        assert!(zip.by_name("body/motions/VRMA_01.vrma").is_ok());
    }

    #[test]
    fn pack_named_from_uses_requested_id() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("AliciaSolid.vrm"), b"vrm-bytes").unwrap();
        let bytes =
            pack_named_from(dir.path(), BUNDLED_ALICIA_B_ID, BUNDLED_ALICIA_B_NAME).expect("pack");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut manifest = String::new();
        zip.by_name("manifest.toml")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(manifest.contains(&format!("id = \"{BUNDLED_ALICIA_B_ID}\"")));
        assert!(manifest.contains(&format!("display_name = \"{BUNDLED_ALICIA_B_NAME}\"")));
    }
}
