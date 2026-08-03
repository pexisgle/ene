//! Character card enumeration and name-to-path resolution.
//!
//! Both discovery ([`discover_characters`]) and resolution
//! ([`resolve_character_path`]) live here so host apps share one rule set.
//! A character is a subdirectory of `assets/characters/` containing
//! `character.json`; resolution additionally falls back to a
//! `characters/{name}.charx` or `characters/{name}.png` file so cards in
//! those containers can be loaded without an import. A character that shows
//! up in a list always resolves.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::CharacterCardV3;
use crate::CharacterConfig;
use crate::card_import::load_card_from_path;
use crate::character_assets::{
    DEFAULT_VRM_PATH, DEFAULT_VRMA_PATH, EneAssetKind, ResolvedAssetUri, resolve_asset_uri,
};
use crate::error::EneConfigError;
use crate::paths;

/// Maximum recursion depth for the legacy extension-scan fallback.
const MAX_SCAN_DEPTH: usize = 16;

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
    /// Motion names (card-declared asset names or file stems), aligned with
    /// `motion_paths`.
    pub motion_names: Vec<String>,
    /// Card path relative to the assets directory.
    pub card_path: String,
    /// The default motion from `character_settings.json`, if any.
    pub default_motion: Option<String>,
}

/// Enumerates the characters under `assets_dir/characters/`.
///
/// A folder counts as a character when it contains `character.json`; folders
/// without one are skipped. VRM/motion resolution is declaration-based when
/// the card lists `assets` (`x_vrm` / `x_vrma`), otherwise the legacy
/// extension scan over the folder (recursively, symlinks excluded) applies.
/// Entries are sorted by name.
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
        let card = load_card_from_path(&card_path).ok();
        let name = card
            .as_ref()
            .map(|c| c.data.name.as_str())
            .filter(|n| !n.is_empty())
            .unwrap_or(folder.as_str())
            .to_string();
        let default_motion_name = read_default_motion(assets_dir, &folder);
        let has_assets = card.as_ref().is_some_and(|c| !c.data.assets.is_empty());

        let vrm_paths = if has_assets {
            declared_asset_paths(card.as_ref(), EneAssetKind::Vrm, &path, assets_dir)
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        } else {
            scan_assets(&path, &folder, "vrm")
        };

        let (motion_paths, motion_names) = if let Some(catalog) = card
            .as_ref()
            .and_then(|c| c.data.extensions.ene.as_ref())
            .and_then(|ene| ene.motion_catalog.as_ref())
        {
            let motion_paths = catalog
                .motions
                .iter()
                .map(|m| format!("characters/{folder}/{}", m.path))
                .collect::<Vec<_>>();
            let motion_names = motion_names_from_paths(&motion_paths);
            (motion_paths, motion_names)
        } else if has_assets {
            let declared =
                declared_asset_paths(card.as_ref(), EneAssetKind::Vrma, &path, assets_dir);
            let motion_paths = declared.iter().map(|(path, _)| path.clone()).collect();
            let motion_names = declared
                .iter()
                .map(|(path, name)| {
                    if name.is_empty() {
                        file_stem(path)
                    } else {
                        name.clone()
                    }
                })
                .collect();
            (motion_paths, motion_names)
        } else {
            let motion_paths = scan_assets(&path, &folder, "vrma");
            let motion_names = motion_names_from_paths(&motion_paths);
            (motion_paths, motion_names)
        };

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
        let folder_card = paths::character_dir_in(assets_dir, trimmed).join("character.json");
        if folder_card.exists() {
            return Ok(folder_card);
        }
        for extension in ["charx", "png"] {
            let candidate = assets_dir
                .join("characters")
                .join(format!("{trimmed}.{extension}"));
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        return Ok(folder_card);
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

/// Resolves declared `x_vrm` / `x_vrma` assets to relative paths plus the
/// card-declared asset name. `embeded://` paths resolve against the card
/// directory; `ccdefault:` maps to the bundled default; remote and data URLs
/// are not playable from discovery and are skipped with a warning.
fn declared_asset_paths(
    card: Option<&CharacterCardV3>,
    kind: EneAssetKind,
    card_dir: &Path,
    assets_dir: &Path,
) -> Vec<(String, String)> {
    let Some(card) = card else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for asset in &card.data.assets {
        if asset.ene_kind() != Some(kind) {
            continue;
        }
        let entry_path = match resolve_asset_uri(&asset.uri) {
            Ok(ResolvedAssetUri::Embedded(path)) => {
                let full = card_dir.join(path);
                let Some(relative) = full.strip_prefix(assets_dir).ok() else {
                    continue;
                };
                if !full.exists() {
                    tracing::warn!(
                        path = %full.display(),
                        asset = %asset.name,
                        "Declared asset is missing on disk"
                    );
                    continue;
                }
                relative.to_string_lossy().to_string()
            }
            Ok(ResolvedAssetUri::AppDefault) => {
                let default = match kind {
                    EneAssetKind::Vrm => DEFAULT_VRM_PATH,
                    EneAssetKind::Vrma => DEFAULT_VRMA_PATH,
                };
                if !assets_dir.join(default).exists() {
                    tracing::warn!(asset = %asset.name, "Default asset is missing on disk");
                    continue;
                }
                default.to_string()
            }
            Ok(ResolvedAssetUri::Remote(_)) => {
                tracing::warn!(asset = %asset.name, "Skipping remote asset at discovery; import it to materialize");
                continue;
            }
            Ok(ResolvedAssetUri::Data { .. }) => {
                tracing::warn!(asset = %asset.name, "Skipping data-URL asset at discovery; import it to materialize");
                continue;
            }
            Err(e) => {
                tracing::warn!(asset = %asset.name, error = %e, "Skipping undeclared asset");
                continue;
            }
        };
        out.push((entry_path, asset.name.clone()));
    }
    out
}

/// Reads the per-character default motion from `character_settings.json`.
fn read_default_motion(assets_dir: &Path, folder: &str) -> Option<String> {
    let settings_path = paths::character_settings_path_in(assets_dir, folder);
    if settings_path.exists() {
        let s = std::fs::read_to_string(settings_path).ok()?;
        let per: CharacterConfig = serde_json::from_str(&s).ok()?;
        if !per.default_motion.is_empty() {
            return Some(per.default_motion);
        }
    }
    None
}

/// Recursive extension scan for legacy cards without `assets` declarations.
/// Symlinks are never followed so a card cannot smuggle a read outside its
/// directory through the fallback path.
fn scan_assets(card_dir: &Path, folder: &str, extension: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![(card_dir.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= MAX_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push((path, depth + 1));
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
                && let Ok(relative) = path.strip_prefix(card_dir)
            {
                found.push(format!(
                    "characters/{folder}/{}",
                    relative.to_string_lossy()
                ));
            }
        }
    }
    found.sort();
    found
}

fn motion_names_from_paths(paths: &[String]) -> Vec<String> {
    paths.iter().map(|path| file_stem(path)).collect()
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Loads a [`CharacterCardV3`] from a resolved path (or bare character name).
///
/// Host apps (`ene-cli`, `ene-desktop`) load the card via this helper (or their
/// own I/O) and pass it to [`ene_runtime::EneHandle::open`] — the runtime does
/// not perform character-card file I/O on the product path. JSON, CHARX
/// (zip), and PNG (ccv3/chara chunk) cards are all accepted.
///
/// # Errors
///
/// Propagates [`EneConfigError::CharacterNotConfigured`] and
/// [`EneConfigError::UnsafeCharacterPath`] from [`resolve_character_path`],
/// plus read and parse errors for the card file itself.
pub fn load_character_card(name_or_path: &str) -> Result<CharacterCardV3, EneConfigError> {
    let path = resolve_character_path(name_or_path)?;
    load_card_from_path(&path)
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

    fn v3_card(name: &str, assets: &str) -> String {
        format!(
            r#"{{"spec":"chara_card_v3","spec_version":"3.0","data":{{"name":"{name}","assets":{assets}}}}}"#
        )
    }

    #[test]
    fn declared_assets_drive_discovery() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"embeded://model.vrm","name":"Model","ext":"vrm"},{"type":"x_vrma","uri":"embeded://motions/wave.vrma","name":"wave","ext":"vrma"}]"#,
            ),
        );
        std::fs::create_dir_all(dir.join("motions")).expect("create motions dir");
        std::fs::write(dir.join("model.vrm"), "x").expect("write vrm");
        std::fs::write(dir.join("motions/wave.vrma"), "x").expect("write vrma");
        std::fs::write(dir.join("loose.vrm"), "x").expect("write loose vrm");

        let found = discover_characters(&assets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].vrm_paths, ["characters/ada/model.vrm"]);
        assert_eq!(found[0].motion_paths, ["characters/ada/motions/wave.vrma"]);
        assert_eq!(found[0].motion_names, ["wave"]);
    }

    #[test]
    fn declared_assets_are_authoritative_over_scan() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"embeded://model.vrm","name":"Model","ext":"vrm"}]"#,
            ),
        );
        std::fs::write(dir.join("model.vrm"), "x").expect("write vrm");
        std::fs::write(dir.join("loose.vrm"), "x").expect("write loose vrm");

        let found = discover_characters(&assets);
        assert_eq!(found[0].vrm_paths, ["characters/ada/model.vrm"]);
        assert!(
            found[0].motion_paths.is_empty(),
            "declared assets must not fall back to scanning"
        );
    }

    #[test]
    fn missing_declared_asset_is_skipped() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"embeded://missing.vrm","name":"Model","ext":"vrm"}]"#,
            ),
        );

        let found = discover_characters(&assets);
        assert!(found[0].vrm_paths.is_empty());
    }

    #[test]
    fn ccdefault_assets_resolve_to_bundled_defaults() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"ccdefault:","name":"Default","ext":"vrm"},{"type":"x_vrma","uri":"ccdefault:","name":"DefaultMotion","ext":"vrma"}]"#,
            ),
        );
        std::fs::create_dir_all(assets.join("characters/Alicia/motions")).expect("create alicia");
        std::fs::write(assets.join(DEFAULT_VRM_PATH), "x").expect("write default vrm");
        std::fs::write(assets.join(DEFAULT_VRMA_PATH), "x").expect("write default vrma");

        let found = discover_characters(&assets);
        assert_eq!(found[0].vrm_paths, ["characters/Alicia/AliciaSolid.vrm"]);
        assert_eq!(
            found[0].motion_paths,
            ["characters/Alicia/motions/VRMA_01.vrma"]
        );
    }

    #[test]
    fn remote_and_data_url_assets_are_skipped_at_discovery() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"https://example.com/model.vrm","name":"Remote","ext":"vrm"},{"type":"x_vrm","uri":"data:model/vrm;base64,QUJD","name":"Inline","ext":"vrm"}]"#,
            ),
        );

        let found = discover_characters(&assets);
        assert!(found[0].vrm_paths.is_empty());
    }

    #[test]
    fn unsafe_declared_uri_is_skipped_at_discovery() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            &v3_card(
                "Ada",
                r#"[{"type":"x_vrm","uri":"embeded://../secret.vrm","name":"Evil","ext":"vrm"}]"#,
            ),
        );

        let found = discover_characters(&assets);
        assert!(found[0].vrm_paths.is_empty());
    }

    #[test]
    fn motion_catalog_precedes_declared_motion_assets() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(
            &dir,
            r#"{
                "spec":"chara_card_v3",
                "spec_version":"3.0",
                "data":{
                    "name":"Ada",
                    "assets":[{"type":"x_vrma","uri":"embeded://motions/wave.vrma","name":"wave","ext":"vrma"}],
                    "extensions":{"ene":{"motion_catalog":{"motions":[{"name":"catalog","path":"motions/catalog.vrma"}]}}}
                }
            }"#,
        );
        std::fs::create_dir_all(dir.join("motions")).expect("create motions dir");
        std::fs::write(dir.join("motions/wave.vrma"), "x").expect("write wave");
        std::fs::write(dir.join("motions/catalog.vrma"), "x").expect("write catalog");

        let found = discover_characters(&assets);
        assert_eq!(
            found[0].motion_paths,
            ["characters/ada/motions/catalog.vrma"]
        );
    }

    #[test]
    fn fallback_scan_finds_nested_assets() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dir = assets.join("characters/ada");
        write_card(&dir, r#"{"data":{"name":"Ada"}}"#);
        std::fs::create_dir_all(dir.join("models/deep")).expect("create nested dir");
        std::fs::create_dir_all(dir.join("motions")).expect("create motions dir");
        std::fs::write(dir.join("models/deep/model.vrm"), "x").expect("write nested vrm");
        std::fs::write(dir.join("motions/VRMA_01.vrma"), "x").expect("write motion");

        let found = discover_characters(&assets);
        assert_eq!(found[0].vrm_paths, ["characters/ada/models/deep/model.vrm"]);
        assert_eq!(
            found[0].motion_paths,
            ["characters/ada/motions/VRMA_01.vrma"]
        );
    }

    #[test]
    fn bare_name_resolves_charx_and_png_fallbacks() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(assets.join("characters")).expect("create characters dir");
        std::fs::write(assets.join("characters/Ada.charx"), b"PK").expect("write charx");
        std::fs::write(assets.join("characters/Ada.png"), b"png").expect("write png");

        let resolved = resolve_character_path_in(&assets, "Ada").expect("charx resolves");
        assert_eq!(resolved, assets.join("characters/Ada.charx"));

        std::fs::remove_file(assets.join("characters/Ada.charx")).expect("remove charx");
        let resolved = resolve_character_path_in(&assets, "Ada").expect("png resolves");
        assert_eq!(resolved, assets.join("characters/Ada.png"));
    }

    #[test]
    fn character_folder_takes_precedence_over_container_files() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        write_card(&assets.join("characters/Ada"), r#"{"data":{"name":"Ada"}}"#);
        std::fs::write(assets.join("characters/Ada.charx"), b"PK").expect("write charx");

        let resolved = resolve_character_path_in(&assets, "Ada").expect("folder wins");
        assert_eq!(resolved, assets.join("characters/Ada/character.json"));
    }
}
