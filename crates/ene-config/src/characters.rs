//! Character card enumeration and name-to-path resolution.
//!
//! Both discovery ([`discover_characters`]) and resolution
//! ([`resolve_character_path`]) live here so host apps share one rule set:
//! a character is a subdirectory of `assets/characters/` containing
//! `character.json`. Enumeration and resolution read that single filename,
//! so a character that shows up in a list always resolves.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::CharacterCardV3;
use crate::CharacterConfig;
use crate::error::EneConfigError;
use crate::paths;

/// A character discovered under `assets/characters/{folder}/`.
///
/// The path fields are relative to the assets directory
/// (e.g. `characters/Alicia/character.json`).
#[derive(Debug, Clone, Serialize)]
pub struct CharacterEntry {
    /// Display name from the card's `data.name`, falling back to the folder name.
    pub name: String,
    /// Folder name under `assets/characters/`.
    pub folder: String,
    /// VRM model paths relative to the assets directory.
    pub vrm_paths: Vec<String>,
    /// VRMA motion paths relative to the assets directory.
    pub motion_paths: Vec<String>,
    /// Motion file stems, aligned with `motion_paths`.
    pub motion_names: Vec<String>,
    /// Card path relative to the assets directory.
    pub card_path: String,
    /// The default motion from `character_settings.json`, if any.
    pub default_motion: Option<String>,
}

/// Enumerates the characters under `assets_dir/characters/`.
///
/// A folder counts as a character when it contains `character.json`; folders
/// without one are skipped. Entries are sorted by name.
#[must_use]
pub fn discover_characters(assets_dir: &Path) -> Vec<CharacterEntry> {
    let mut out = Vec::new();
    let characters_dir = assets_dir.join("characters");
    let Ok(dir) = std::fs::read_dir(&characters_dir) else {
        return out;
    };
    for entry in dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to read character directory entry");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(folder_name_os) = path.file_name() else {
            continue;
        };
        let folder = folder_name_os.to_string_lossy().to_string();
        let card_path = path.join("character.json");
        if !card_path.exists() {
            continue;
        }
        let (name, default_motion_name, card_motions) =
            read_character_json_meta(&card_path, assets_dir, &folder)
                .unwrap_or_else(|| (folder.clone(), None, None));

        let mut vrm_paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&path) {
            for file in entries {
                let file = match file {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %path.display(), "Failed to read file in character dir");
                        continue;
                    }
                };
                let file_path = file.path();
                if file_path.is_dir() {
                    continue;
                }
                let Some(file_name_os) = file_path.file_name() else {
                    continue;
                };
                let relative = format!("characters/{folder}/{}", file_name_os.to_string_lossy());
                if file_path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("vrm"))
                {
                    vrm_paths.push(relative);
                }
            }
        }
        vrm_paths.sort();

        let mut motion_paths = Vec::new();
        if let Some(card_motions) = card_motions {
            for m in card_motions {
                motion_paths.push(format!("characters/{folder}/{}", m.path));
            }
        } else {
            let motions_dir = path.join("motions");
            if let Ok(entries) = std::fs::read_dir(&motions_dir) {
                for file in entries {
                    let file = match file {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!(error = %e, path = %motions_dir.display(), "Failed to read motion file");
                            continue;
                        }
                    };
                    let file_path = file.path();
                    if file_path.is_dir() {
                        continue;
                    }
                    let Some(file_name_os) = file_path.file_name() else {
                        continue;
                    };
                    let relative = format!(
                        "characters/{folder}/motions/{}",
                        file_name_os.to_string_lossy()
                    );
                    if file_path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("vrma"))
                    {
                        motion_paths.push(relative);
                    }
                }
            }
        }
        motion_paths.sort();

        let motion_names = motion_paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        out.push(CharacterEntry {
            name,
            folder: folder.clone(),
            vrm_paths,
            motion_names,
            motion_paths,
            card_path: format!("characters/{folder}/character.json"),
            default_motion: default_motion_name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Resolves a character name to a full card path.
///
/// A bare name maps to `assets_dir/characters/{name}/character.json`; a value
/// containing a path separator is treated as a card path (relative or
/// absolute). Paths with `..` traversal components are rejected, since
/// character names come from third-party card distributions.
///
/// # Errors
///
/// - [`EneConfigError::CharacterNotConfigured`] when `name` is empty;
/// - [`EneConfigError::UnsafeCharacterPath`] for paths with `..` traversal
///   components.
pub fn resolve_character_path(name: &str) -> Result<PathBuf, EneConfigError> {
    resolve_character_path_in(crate::paths::assets_dir(), name)
}

/// `resolve_character_path` for an explicit base assets directory.
pub(crate) fn resolve_character_path_in(
    assets_dir: &Path,
    name: &str,
) -> Result<PathBuf, EneConfigError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(EneConfigError::CharacterNotConfigured);
    }
    if contains_traversal(trimmed) {
        return Err(EneConfigError::UnsafeCharacterPath(trimmed.to_string()));
    }
    if !has_path_separator(trimmed) {
        return Ok(paths::character_dir_in(assets_dir, trimmed).join("character.json"));
    }
    Ok(PathBuf::from(trimmed))
}

fn has_path_separator(name: &str) -> bool {
    name.contains(['/', '\\'])
}

/// `true` when any component (split on `/` or `\`) is `..`.
fn contains_traversal(name: &str) -> bool {
    let is_separator = |c: char| c == '/' || c == '\\';
    name.split(is_separator).any(|component| component == "..")
}

/// Reads the card's display name, per-character default motion, and the
/// motion catalog from `extensions.ene.motion_catalog`.
fn read_character_json_meta(
    card_path: &Path,
    assets_dir: &Path,
    folder: &str,
) -> Option<(String, Option<String>, Option<Vec<crate::MotionEntry>>)> {
    let content = std::fs::read_to_string(card_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = v.get("data")?.get("name")?.as_str()?.to_string();

    let default_motion = (|| {
        let settings_path = paths::character_settings_path_in(assets_dir, folder);
        if settings_path.exists() {
            let s = std::fs::read_to_string(settings_path).ok()?;
            let per: CharacterConfig = serde_json::from_str(&s).ok()?;
            if !per.default_motion.is_empty() {
                return Some(per.default_motion);
            }
        }
        None
    })();

    let motions = (|| {
        let extensions = v.get("data")?.get("extensions")?;
        let ene = extensions.get("ene")?;
        let catalog = ene.get("motion_catalog")?;
        let motions_val = catalog.get("motions")?;
        let motions: Vec<crate::MotionEntry> = serde_json::from_value(motions_val.clone()).ok()?;
        Some(motions)
    })();

    Some((name, default_motion, motions))
}

/// Loads a [`CharacterCardV3`] from a resolved path (or bare character name).
///
/// Host apps (`ene-cli`, `ene-desktop`) load the card via this helper (or their
/// own I/O) and pass it to [`ene_runtime::EneHandle::open`] — the runtime does
/// not perform character-card file I/O on the product path.
///
/// # Errors
///
/// Propagates [`EneConfigError::CharacterNotConfigured`] and
/// [`EneConfigError::UnsafeCharacterPath`] from [`resolve_character_path`],
/// plus read and parse errors for the card file itself.
pub fn load_character_card(name_or_path: &str) -> Result<CharacterCardV3, EneConfigError> {
    let path = resolve_character_path(name_or_path)?;
    let file_content = std::fs::read_to_string(&path).map_err(EneConfigError::CardReadError)?;
    serde_json::from_str(&file_content).map_err(EneConfigError::JsonError)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_card(dir: &Path, content: &str) {
        std::fs::create_dir_all(dir).expect("create character dir");
        std::fs::write(dir.join("character.json"), content).expect("write character card");
    }

    #[test]
    fn bare_name_resolves_under_characters_dir() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let resolved = resolve_character_path_in(tmp.path(), "Alicia").expect("bare name resolves");
        assert_eq!(
            resolved,
            tmp.path()
                .join("characters")
                .join("Alicia")
                .join("character.json")
        );
    }

    #[test]
    fn empty_name_is_not_configured() {
        let tmp = tempfile::tempdir().expect("temp dir");
        for name in ["", "   ", "\t"] {
            assert!(
                matches!(
                    resolve_character_path_in(tmp.path(), name),
                    Err(EneConfigError::CharacterNotConfigured)
                ),
                "expected CharacterNotConfigured for {name:?}"
            );
        }
    }

    #[test]
    fn traversal_names_are_rejected() {
        let tmp = tempfile::tempdir().expect("temp dir");
        for name in [
            "..",
            "../evil",
            "characters/../evil/character.json",
            "sub/..",
            "..\\evil",
            "Alicia\\..\\..\\secret",
        ] {
            assert!(
                matches!(
                    resolve_character_path_in(tmp.path(), name),
                    Err(EneConfigError::UnsafeCharacterPath(_))
                ),
                "expected UnsafeCharacterPath for {name:?}"
            );
        }
    }

    #[test]
    fn absolute_path_resolves_as_is() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let absolute = tmp.path().join("cards").join("ene.json");
        let resolved = resolve_character_path_in(tmp.path(), &absolute.to_string_lossy())
            .expect("absolute path resolves");
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn safe_relative_path_resolves_as_is() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let resolved = resolve_character_path_in(tmp.path(), "assets/cards/ene.json")
            .expect("safe relative path resolves");
        assert_eq!(resolved, PathBuf::from("assets/cards/ene.json"));
    }

    #[test]
    fn discovers_character_folders_sorted_by_name() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        write_card(
            &assets.join("characters/zeta"),
            r#"{"data":{"name":"Zeta"}}"#,
        );
        write_card(
            &assets.join("characters/alpha"),
            r#"{"data":{"name":"Alpha"}}"#,
        );
        std::fs::create_dir_all(assets.join("characters/no_card")).expect("create empty dir");

        let found = discover_characters(&assets);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Zeta"]);
        assert_eq!(found[0].card_path, "characters/alpha/character.json");
        assert_eq!(found[0].folder, "alpha");
    }

    #[test]
    fn typo_card_filename_is_not_discovered() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let typo_dir = assets.join("characters/typo");
        std::fs::create_dir_all(&typo_dir).expect("create character dir");
        std::fs::write(typo_dir.join("charactor.json"), "{}").expect("write typo-named card");

        assert!(discover_characters(&assets).is_empty());
    }

    #[test]
    fn unreadable_card_falls_back_to_folder_name() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        write_card(&assets.join("characters/ghost"), "{not json");

        let found = discover_characters(&assets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "ghost");
        assert_eq!(found[0].default_motion, None);
    }

    #[test]
    fn default_motion_and_media_paths_are_discovered() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/dance");
        write_card(&dir, r#"{"data":{"name":"Dance"}}"#);
        std::fs::write(
            dir.join("character_settings.json"),
            r#"{"default_motion":"wave"}"#,
        )
        .expect("write character settings");
        std::fs::create_dir_all(dir.join("motions")).expect("create motions dir");
        std::fs::write(dir.join("motions/VRMA_01.vrma"), "x").expect("write motion");
        std::fs::write(dir.join("model.vrm"), "x").expect("write vrm");

        let found = discover_characters(&assets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Dance");
        assert_eq!(found[0].default_motion.as_deref(), Some("wave"));
        assert_eq!(found[0].motion_names, ["VRMA_01"]);
        assert_eq!(
            found[0].motion_paths,
            ["characters/dance/motions/VRMA_01.vrma"]
        );
        assert_eq!(found[0].vrm_paths, ["characters/dance/model.vrm"]);
    }

    /// Every enumerated character must resolve through the same rule that
    /// enumerated it, to an existing card file.
    #[test]
    fn every_discovered_character_resolves() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        write_card(&assets.join("characters/one"), r#"{"data":{"name":"One"}}"#);
        write_card(&assets.join("characters/two"), r#"{"data":{"name":"Two"}}"#);

        let found = discover_characters(&assets);
        assert_eq!(found.len(), 2);
        for entry in &found {
            let resolved = resolve_character_path_in(&assets, &entry.folder)
                .expect("discovered character must resolve");
            assert_eq!(resolved, assets.join(&entry.card_path));
            assert!(resolved.exists());
        }
    }

    #[test]
    fn load_card_reports_unset_and_unsafe_names() {
        assert!(matches!(
            load_character_card(""),
            Err(EneConfigError::CharacterNotConfigured)
        ));
        assert!(matches!(
            load_character_card("../evil"),
            Err(EneConfigError::UnsafeCharacterPath(_))
        ));
    }
}
