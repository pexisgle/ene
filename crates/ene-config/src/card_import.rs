//! Character card containers: PNG `ccv3`/`chara` text chunks and CHARX zips,
//! plus materializing imports into `assets/characters/`.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::Serialize;
use zip::ZipArchive;

use crate::CharacterAsset;
use crate::CharacterCardV3;
use crate::character_assets::{ResolvedAssetUri, decode_data_payload, resolve_asset_uri};
use crate::error::EneConfigError;
use crate::locale::{LocalizedCharacterFields, merge_localized_fields, strip_locales};
use crate::paths;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const MAX_PNG_CHUNKS: usize = 4_096;
/// Decompressed zTXt/iTXt cap: card text is at most a few hundred KB even
/// for lore-heavy cards; this only bounds hostile archives.
const MAX_PNG_TEXT_BYTES: u64 = 64 * 1024 * 1024;
/// Per-entry CHARX extraction cap. VRM models stay under a few hundred MB;
/// the spec lets applications reject overlarge archives.
const MAX_CHARX_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
/// Total CHARX extraction cap (decompression-bomb protection).
const MAX_CHARX_TOTAL_BYTES: u64 = 4 * MAX_CHARX_ENTRY_BYTES;
/// Data-URL asset materialization cap (base64 inflates by ~1/3 in memory).
const MAX_DATA_URI_ASSET_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum characters kept in an import-derived folder/file name.
const MAX_IMPORT_NAME_CHARS: usize = 64;
/// Whole-file cap applied before a card file is read into memory.
const MAX_CARD_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// Result of a successful character card import.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedCharacter {
    /// Display name from the card.
    pub name: String,
    /// Folder name under `assets/characters/`.
    pub folder: String,
    /// Card path relative to the assets directory.
    pub card_path: String,
}

/// Imports a PNG or CHARX character card into `assets/characters/`.
///
/// PNG cards materialize as `{card name}/character.json` (the PNG itself is
/// kept as `avatar.png` so a `ccdefault:` icon resolves to a real file).
/// CHARX archives are extracted entry-by-entry with path and size validation.
///
/// # Errors
///
/// Returns [`EneConfigError::UnsupportedCardFile`] for files that are not
/// PNG or CHARX, container-specific errors for corrupt archives, and
/// [`EneConfigError::CharacterImportExists`] when the target folder already
/// exists (imports never overwrite).
pub fn import_character_file(src: &Path) -> Result<ImportedCharacter, EneConfigError> {
    import_character_file_in(src, paths::assets_dir())
}

/// `import_character_file` for an explicit base assets directory.
pub(crate) fn import_character_file_in(
    src: &Path,
    assets_dir: &Path,
) -> Result<ImportedCharacter, EneConfigError> {
    let bytes = read_card_file(src)?;
    let extension = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => import_png(&bytes, src, assets_dir),
        Some("charx") => import_charx(&bytes, src, assets_dir),
        Some("json") => Err(EneConfigError::UnsupportedCardFile(
            "JSON cards already live at characters/{name}/character.json; place the file there"
                .to_string(),
        )),
        _ => Err(EneConfigError::UnsupportedCardFile(
            src.display().to_string(),
        )),
    }
}

/// Reads a card from a resolved path, sniffing PNG / CHARX / JSON content.
pub(crate) fn load_card_from_path(path: &Path) -> Result<CharacterCardV3, EneConfigError> {
    let bytes = read_card_file(path)?;
    load_card_from_bytes(&bytes)
}

/// Reads a card with `code`'s diff layered over the base.
///
/// Folder-form cards (`character.json`) additionally consult the
/// `character.{code}.json` sidecar next to the card; CHARX reads the same
/// entry name from the archive root and PNG cards use the embedded
/// `extensions.ene.locales` bag.
pub(crate) fn load_card_from_path_localized(
    path: &Path,
    code: &str,
) -> Result<CharacterCardV3, EneConfigError> {
    let bytes = read_card_file(path)?;
    let code = crate::resolve_language_alias(code);
    let mut card = load_card_from_bytes_localized(&bytes, &code)?;
    if !is_container(&bytes)
        && path
            .file_name()
            .is_some_and(|name| name == "character.json")
        && let Some(diff) = read_sidecar_diff(path.parent(), &code)
    {
        merge_localized_fields(&mut card, &diff);
    }
    Ok(card)
}

fn read_card_file(path: &Path) -> Result<Vec<u8>, EneConfigError> {
    let metadata = std::fs::metadata(path).map_err(EneConfigError::CardReadError)?;
    if metadata.len() > MAX_CARD_FILE_BYTES {
        return Err(EneConfigError::CardFileTooLarge(MAX_CARD_FILE_BYTES));
    }
    std::fs::read(path).map_err(EneConfigError::CardReadError)
}

/// Parses card bytes by magic: PNG signature, zip signature, else JSON.
pub(crate) fn load_card_from_bytes(bytes: &[u8]) -> Result<CharacterCardV3, EneConfigError> {
    parse_card_bytes(bytes, None)
}

/// `load_card_from_bytes` plus locale layering and normalization.
pub(crate) fn load_card_from_bytes_localized(
    bytes: &[u8],
    code: &str,
) -> Result<CharacterCardV3, EneConfigError> {
    let code = crate::resolve_language_alias(code);
    let mut card = parse_card_bytes(bytes, Some(&code))?;
    strip_locales(&mut card);
    Ok(card)
}

/// Parses card bytes by magic and, when `code` is given, layers the locale
/// diff carried inside the container (PNG bag or CHARX root entry).
fn parse_card_bytes(bytes: &[u8], code: Option<&str>) -> Result<CharacterCardV3, EneConfigError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        let mut card: CharacterCardV3 =
            serde_json::from_value(png_card_json(bytes)?).map_err(EneConfigError::JsonError)?;
        if let Some(code) = code {
            merge_embedded_locale(&mut card, code);
        }
        return Ok(card);
    }
    if is_zip_archive(bytes) {
        if let Some(code) = code {
            let (value, diff) = charx_card_json_localized(bytes, Some(code))?;
            let mut card: CharacterCardV3 =
                serde_json::from_value(value).map_err(EneConfigError::JsonError)?;
            if let Some(diff) = diff {
                merge_localized_fields(&mut card, &diff);
            }
            return Ok(card);
        }
        return serde_json::from_value(charx_card_json(bytes)?).map_err(EneConfigError::JsonError);
    }
    let mut card: CharacterCardV3 =
        serde_json::from_slice(bytes).map_err(EneConfigError::JsonError)?;
    if let Some(code) = code {
        merge_embedded_locale(&mut card, code);
    }
    Ok(card)
}

fn is_zip_archive(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

fn is_container(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_SIGNATURE) || is_zip_archive(bytes)
}

/// Layers the embedded `extensions.ene.locales` diff for `code` over `card`.
///
/// Locale keys are canonicalized with [`crate::resolve_language_alias`], so
/// a producer embedding `ja-JP` is read as `ja`.
fn merge_embedded_locale(card: &mut CharacterCardV3, code: &str) {
    let diff = card
        .data
        .extensions
        .ene
        .as_ref()
        .and_then(|ene| ene.locales.as_ref())
        .and_then(|locales| {
            locales
                .iter()
                .find(|(key, _)| crate::resolve_language_alias(key) == code)
                .map(|(_, diff)| diff.clone())
        });
    if let Some(diff) = diff {
        merge_localized_fields(card, &diff);
    }
}

/// Reads the `character.{code}.json` sidecar next to a folder-form card.
///
/// A missing file is the normal case and yields `None`; a malformed or
/// oversized diff is warned about and skipped so a broken translation never
/// sinks the base card.
fn read_sidecar_diff(card_dir: Option<&Path>, code: &str) -> Option<LocalizedCharacterFields> {
    let dir = card_dir?;
    let path = dir.join(format!("character.{code}.json"));
    let bytes = match read_card_file(&path) {
        Ok(bytes) => bytes,
        Err(EneConfigError::CardFileTooLarge(_)) => {
            tracing::warn!(path = %path.display(), "Skipping oversized localized card diff");
            return None;
        }
        Err(_) => return None,
    };
    match serde_json::from_slice(&bytes) {
        Ok(diff) => Some(diff),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Skipping malformed localized card diff"
            );
            None
        }
    }
}

// ── PNG cards ──

/// Extracts the card JSON from a PNG `ccv3` (V3) or `chara` (V2) chunk.
///
/// The spec mandates a tEXt chunk with base64-encoded JSON; zTXt and iTXt
/// (compressed / UTF-8 variants) are accepted for real-world cards, as is
/// plain JSON for producers that skip the base64 step.
fn png_card_json(bytes: &[u8]) -> Result<serde_json::Value, EneConfigError> {
    let chunks = png_text_chunks(bytes)?;
    let ccv3 = chunks.iter().find(|chunk| chunk.keyword == "ccv3");
    let chara = chunks.iter().find(|chunk| chunk.keyword == "chara");
    let Some(chunk) = ccv3.or(chara) else {
        return Err(EneConfigError::PngCardMissingChunk);
    };
    let value = decode_chunk_json(&chunk.payload)?;
    if ccv3.is_some() {
        return Ok(value);
    }
    // V2 cards wrap into the V3 shape; every missing `data` field takes its
    // serde default, so the legacy schema parses as-is.
    Ok(serde_json::json!({
        "spec": "chara_card_v3",
        "spec_version": "3.0",
        "data": value
    }))
}

struct TextChunk {
    keyword: String,
    payload: Vec<u8>,
}

fn png_text_chunks(bytes: &[u8]) -> Result<Vec<TextChunk>, EneConfigError> {
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err(EneConfigError::InvalidPngCard(
            "missing PNG signature".to_string(),
        ));
    }
    let mut offset = PNG_SIGNATURE.len();
    let mut out = Vec::new();
    let mut chunks_seen = 0usize;
    while offset < bytes.len() {
        chunks_seen += 1;
        if chunks_seen > MAX_PNG_CHUNKS {
            return Err(EneConfigError::InvalidPngCard(
                "too many chunks".to_string(),
            ));
        }
        let Some(len_bytes) = bytes.get(offset..offset + 4) else {
            return Err(EneConfigError::InvalidPngCard(
                "truncated chunk header".to_string(),
            ));
        };
        let len = u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        let data_start = offset + 8;
        let data_end = data_start + len as usize;
        let crc_end = data_end + 4;
        if crc_end > bytes.len() {
            return Err(EneConfigError::InvalidPngCard(
                "truncated chunk data".to_string(),
            ));
        }
        let chunk_type = &bytes[offset + 4..data_start];
        let data = &bytes[data_start..data_end];
        match chunk_type {
            b"tEXt" => {
                if let Some((keyword, payload)) = parse_text_chunk(data) {
                    out.push(TextChunk {
                        keyword,
                        payload: payload.to_vec(),
                    });
                }
            }
            b"zTXt" => {
                if let Some(chunk) = parse_ztext_chunk(data) {
                    out.push(chunk);
                }
            }
            b"iTXt" => {
                if let Some(chunk) = parse_itext_chunk(data) {
                    out.push(chunk);
                }
            }
            b"IEND" => break,
            _ => {}
        }
        offset = crc_end;
    }
    Ok(out)
}

fn parse_text_chunk(data: &[u8]) -> Option<(String, &[u8])> {
    let nul = data.iter().position(|byte| *byte == 0)?;
    let keyword = String::from_utf8_lossy(&data[..nul]).into_owned();
    if keyword.is_empty() || keyword.len() > 79 {
        return None;
    }
    Some((keyword, &data[nul + 1..]))
}

fn parse_ztext_chunk(data: &[u8]) -> Option<TextChunk> {
    let (keyword, rest) = parse_text_chunk(data)?;
    let (method, compressed) = rest.split_first()?;
    if *method != 0 {
        return None;
    }
    let payload = inflate(compressed, MAX_PNG_TEXT_BYTES).ok()?;
    Some(TextChunk { keyword, payload })
}

fn parse_itext_chunk(data: &[u8]) -> Option<TextChunk> {
    let (keyword, rest) = parse_text_chunk(data)?;
    let (flag, rest) = rest.split_first()?;
    let (_method, rest) = rest.split_first()?;
    let lang_end = rest.iter().position(|byte| *byte == 0)?;
    let rest = &rest[lang_end + 1..];
    let translated_end = rest.iter().position(|byte| *byte == 0)?;
    let text = &rest[translated_end + 1..];
    let payload = match *flag {
        0 => text.to_vec(),
        1 => inflate(text, MAX_PNG_TEXT_BYTES).ok()?,
        _ => return None,
    };
    Some(TextChunk { keyword, payload })
}

fn inflate(data: &[u8], limit: u64) -> Result<Vec<u8>, EneConfigError> {
    // The PNG spec wraps zTXt/iTXt text in a zlib datastream; raw deflate is
    // accepted as a fallback for writers that skip the wrapper.
    match inflate_with(flate2::read::ZlibDecoder::new(data), limit) {
        Ok(out) => Ok(out),
        Err(_) => inflate_with(flate2::read::DeflateDecoder::new(data), limit),
    }
}

fn inflate_with<R: std::io::Read>(decoder: R, limit: u64) -> Result<Vec<u8>, EneConfigError> {
    let mut out = Vec::new();
    decoder
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|_| EneConfigError::InvalidPngCard("invalid compressed text".to_string()))?;
    if out.len() as u64 > limit {
        return Err(EneConfigError::InvalidPngCard(
            "compressed text too large".to_string(),
        ));
    }
    Ok(out)
}

fn decode_chunk_json(payload: &[u8]) -> Result<serde_json::Value, EneConfigError> {
    let text = String::from_utf8_lossy(payload);
    let trimmed = text.trim().trim_end_matches('\0');
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(EneConfigError::JsonError);
    }
    let compact: String = trimmed
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let decoded = STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|_| {
            EneConfigError::InvalidPngCard("text chunk is not valid base64".to_string())
        })?;
    let json = String::from_utf8(decoded)
        .map_err(|_| EneConfigError::InvalidPngCard("base64 payload is not UTF-8".to_string()))?;
    serde_json::from_str(&json).map_err(EneConfigError::JsonError)
}

// ── CHARX cards ──

/// Reads the root `card.json` from a CHARX zip.
fn charx_card_json(bytes: &[u8]) -> Result<serde_json::Value, EneConfigError> {
    charx_card_json_localized(bytes, None).map(|(card, _)| card)
}

/// Reads `card.json` and, when `code` is given, the root
/// `character.{code}.json` diff entry from a CHARX zip.
///
/// Only the requested entries are probed for encryption and size, so a base
/// (unlocalized) load is unaffected by a hostile or corrupt diff entry. A
/// diff entry hard-errors only when encrypted (an archive-integrity
/// boundary); oversized or malformed ones are skipped with a warning, like
/// the folder sidecar path.
fn charx_card_json_localized(
    bytes: &[u8],
    code: Option<&str>,
) -> Result<(serde_json::Value, Option<LocalizedCharacterFields>), EneConfigError> {
    let mut archive =
        ZipArchive::new(std::io::Cursor::new(bytes)).map_err(EneConfigError::CharxError)?;
    let diff_name = code.map(|code| format!("character.{code}.json"));
    let mut card: Option<serde_json::Value> = None;
    let mut diff: Option<LocalizedCharacterFields> = None;
    for index in 0..archive.len() {
        let name = {
            let file = archive
                .by_index_raw(index)
                .map_err(EneConfigError::CharxError)?;
            file.name().to_string()
        };
        let is_diff_entry = diff_name
            .as_deref()
            .is_some_and(|diff_name| diff_name == name.as_str());
        if name != "card.json" && !is_diff_entry {
            continue;
        }
        // `by_index` refuses encrypted entries up front, so the encryption
        // probe goes through `by_index_raw` and the read below re-opens
        // with `by_index`.
        let is_encrypted = {
            let file = archive
                .by_index_raw(index)
                .map_err(EneConfigError::CharxError)?;
            file.encrypted()
        };
        if is_encrypted {
            return Err(EneConfigError::CharxEncrypted(name));
        }
        let mut file = archive
            .by_index(index)
            .map_err(EneConfigError::CharxError)?;
        let size = file.size();
        if size > MAX_CHARX_ENTRY_BYTES {
            if name == "card.json" {
                return Err(EneConfigError::CharxTooLarge(name));
            }
            tracing::warn!(
                name = %name,
                "Skipping oversized localized diff entry in CHARX archive"
            );
            continue;
        }
        let mut content = Vec::new();
        file.by_ref()
            .take(size + 1)
            .read_to_end(&mut content)
            .map_err(|e| EneConfigError::CharxError(zip::result::ZipError::Io(e)))?;
        if content.len() as u64 > size {
            if name == "card.json" {
                return Err(EneConfigError::CharxTooLarge(name));
            }
            tracing::warn!(
                name = %name,
                "Skipping size-mismatched localized diff entry in CHARX archive"
            );
            continue;
        }
        if name == "card.json" && card.is_none() {
            card = Some(serde_json::from_slice(&content).map_err(EneConfigError::JsonError)?);
        } else if name != "card.json" && diff.is_none() {
            match serde_json::from_slice(&content) {
                Ok(parsed) => diff = Some(parsed),
                Err(e) => tracing::warn!(
                    name = %name,
                    error = %e,
                    "Skipping malformed localized diff entry in CHARX archive"
                ),
            }
        }
        if card.is_some() && (diff_name.is_none() || diff.is_some()) {
            break;
        }
    }
    let card = card.ok_or(EneConfigError::CharxMissingCard)?;
    Ok((card, diff))
}

/// Rejects zip entry names that could escape the extraction directory.
fn validate_zip_entry_name(name: &str) -> Result<(), EneConfigError> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains(['\\', '\0'])
    {
        return Err(EneConfigError::CharxUnsafePath(name.to_string()));
    }
    for component in name.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || is_drive_prefix(component)
        {
            return Err(EneConfigError::CharxUnsafePath(name.to_string()));
        }
    }
    Ok(())
}

fn is_drive_prefix(component: &str) -> bool {
    component.len() >= 2
        && component.as_bytes()[0].is_ascii_alphabetic()
        && component.as_bytes()[1] == b':'
}

// ── Import ──

fn import_png(
    bytes: &[u8],
    src: &Path,
    assets_dir: &Path,
) -> Result<ImportedCharacter, EneConfigError> {
    let mut card: CharacterCardV3 =
        serde_json::from_value(png_card_json(bytes)?).map_err(EneConfigError::JsonError)?;
    validate_card_assets(&card)?;
    let folder = import_folder_name(&card, src)?;
    let target = assets_dir.join("characters").join(&folder);
    if target.exists() {
        return Err(EneConfigError::CharacterImportExists(folder));
    }
    let staging = staging_dir(assets_dir, &folder);
    let outcome = (|| -> Result<ImportedCharacter, EneConfigError> {
        std::fs::create_dir_all(&staging).map_err(EneConfigError::IoError)?;
        materialize_data_assets(&mut card, &staging)?;
        split_embedded_locales(&mut card, &staging)?;
        write_card_json(&card, &staging.join("character.json"))?;
        std::fs::write(staging.join("avatar.png"), bytes).map_err(EneConfigError::IoError)?;
        std::fs::rename(&staging, &target).map_err(EneConfigError::IoError)?;
        Ok(ImportedCharacter {
            name: card.data.get_character_name().to_string(),
            card_path: format!("characters/{folder}/character.json"),
            folder,
        })
    })();
    if outcome.is_err() {
        drop(std::fs::remove_dir_all(&staging));
    }
    outcome
}

fn import_charx(
    bytes: &[u8],
    src: &Path,
    assets_dir: &Path,
) -> Result<ImportedCharacter, EneConfigError> {
    let mut card: CharacterCardV3 =
        serde_json::from_value(charx_card_json(bytes)?).map_err(EneConfigError::JsonError)?;
    validate_card_assets(&card)?;
    let folder = import_folder_name(&card, src)?;
    let target = assets_dir.join("characters").join(&folder);
    if target.exists() {
        return Err(EneConfigError::CharacterImportExists(folder));
    }
    let staging = staging_dir(assets_dir, &folder);
    let outcome = (|| -> Result<ImportedCharacter, EneConfigError> {
        std::fs::create_dir_all(&staging).map_err(EneConfigError::IoError)?;
        let mut archive =
            ZipArchive::new(std::io::Cursor::new(bytes)).map_err(EneConfigError::CharxError)?;
        validate_charx_entries(&mut archive)?;
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .map_err(EneConfigError::CharxError)?;
            let name = file.name().to_string();
            if file.is_dir() {
                continue;
            }
            let size = file.size();
            let mut content = Vec::new();
            file.by_ref()
                .take(size + 1)
                .read_to_end(&mut content)
                .map_err(|e| EneConfigError::CharxError(zip::result::ZipError::Io(e)))?;
            if content.len() as u64 > size {
                return Err(EneConfigError::CharxTooLarge(name));
            }
            let relative = if name == "card.json" {
                Path::new("character.json")
            } else {
                Path::new(&name)
            };
            write_import_entry(&staging, relative, &content)?;
        }
        split_embedded_locales(&mut card, &staging)?;
        materialize_data_assets(&mut card, &staging)?;
        write_card_json(&card, &staging.join("character.json"))?;
        std::fs::rename(&staging, &target).map_err(EneConfigError::IoError)?;
        Ok(ImportedCharacter {
            name: card.data.get_character_name().to_string(),
            card_path: format!("characters/{folder}/character.json"),
            folder,
        })
    })();
    if outcome.is_err() {
        drop(std::fs::remove_dir_all(&staging));
    }
    outcome
}

/// Materializes embedded `extensions.ene.locales` as `character.{code}.json`
/// sidecars and strips them from the card, producing the folder work form.
/// Sidecars already on disk (extracted from a CHARX archive) win only when
/// they parse as a valid diff; a malformed sidecar is overwritten so a
/// broken archive entry cannot discard the embedded translation.
fn split_embedded_locales(
    card: &mut CharacterCardV3,
    card_dir: &Path,
) -> Result<(), EneConfigError> {
    let locales = match card.data.extensions.ene.as_mut() {
        Some(ene) => ene.locales.take(),
        None => return Ok(()),
    };
    let Some(locales) = locales else {
        return Ok(());
    };
    for (key, fields) in locales {
        let code = crate::resolve_language_alias(&key);
        let path = card_dir.join(format!("character.{code}.json"));
        match std::fs::read_to_string(&path) {
            Ok(content) if serde_json::from_str::<LocalizedCharacterFields>(&content).is_ok() => {
                continue;
            }
            Ok(_) => tracing::warn!(
                path = %path.display(),
                "Overwriting malformed localized card diff sidecar with the embedded locale"
            ),
            Err(_) => {}
        }
        let json = serde_json::to_string_pretty(&fields).map_err(EneConfigError::SerializeError)?;
        std::fs::write(&path, json).map_err(EneConfigError::IoError)?;
    }
    if card
        .data
        .extensions
        .ene
        .as_ref()
        .is_some_and(crate::EneExtension::is_empty)
    {
        card.data.extensions.ene = None;
    }
    Ok(())
}

/// Rejects unsafe, encrypted, symlink, and oversized entries before any
/// bytes are written during extraction.
fn validate_charx_entries(
    archive: &mut ZipArchive<std::io::Cursor<&[u8]>>,
) -> Result<(), EneConfigError> {
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(EneConfigError::CharxError)?;
        let name = file.name().to_string();
        validate_zip_entry_name(&name)?;
        if file.is_dir() {
            continue;
        }
        if file.encrypted() {
            return Err(EneConfigError::CharxEncrypted(name));
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(EneConfigError::CharxUnsafePath(name));
        }
        let size = file.size();
        if size > MAX_CHARX_ENTRY_BYTES {
            return Err(EneConfigError::CharxTooLarge(name));
        }
        total = total.saturating_add(size);
        if total > MAX_CHARX_TOTAL_BYTES {
            return Err(EneConfigError::CharxTooLarge(name));
        }
    }
    Ok(())
}

/// Unique staging directory for an atomic import; renamed over the target
/// only after every file is written.
fn staging_dir(assets_dir: &Path, folder: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    assets_dir
        .join("characters")
        .join(format!(".import-{folder}-{}-{nonce}", std::process::id()))
}

fn write_import_entry(
    target: &Path,
    relative: &Path,
    content: &[u8],
) -> Result<(), EneConfigError> {
    let out_path = target.join(relative);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(EneConfigError::IoError)?;
    }
    std::fs::write(out_path, content).map_err(EneConfigError::IoError)
}

fn write_card_json(card: &CharacterCardV3, path: &Path) -> Result<(), EneConfigError> {
    let json = serde_json::to_string_pretty(card).map_err(EneConfigError::SerializeError)?;
    std::fs::write(path, json).map_err(EneConfigError::IoError)
}

/// Hard-rejects unsafe URIs on cards entering the assets tree; unknown
/// schemes are skipped per the spec's MAY-ignore rule.
fn validate_card_assets(card: &CharacterCardV3) -> Result<(), EneConfigError> {
    for asset in &card.data.assets {
        let consumed = asset.ene_kind().is_some();
        match resolve_asset_uri(&asset.uri) {
            Ok(_) => {}
            Err(EneConfigError::UnsupportedAssetUriScheme(scheme)) => {
                tracing::warn!(
                    scheme = %scheme,
                    asset = %asset.name,
                    "Skipping asset with unsupported URI scheme"
                );
            }
            // A malformed URI on a type Ene does not consume is not a
            // security boundary; unsafe paths above still fail the import.
            Err(EneConfigError::InvalidAssetUri(_)) if !consumed => {
                tracing::warn!(
                    asset = %asset.name,
                    "Skipping malformed asset URI on an unconsumed asset type"
                );
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Writes decoded `data:` assets next to the card and rewrites their URIs to
/// `embeded://` so the materialized card is self-contained.
fn materialize_data_assets(
    card: &mut CharacterCardV3,
    target: &Path,
) -> Result<(), EneConfigError> {
    let mut changed = false;
    for (index, asset) in card.data.assets.iter_mut().enumerate() {
        let Some(kind) = asset.ene_kind() else {
            continue;
        };
        let resolved = match resolve_asset_uri(&asset.uri) {
            Ok(resolved) => resolved,
            Err(EneConfigError::UnsupportedAssetUriScheme(_)) => continue,
            Err(e) => return Err(e),
        };
        let ResolvedAssetUri::Data {
            is_base64,
            payload,
            media_type,
        } = resolved
        else {
            continue;
        };
        let bytes = decode_data_payload(is_base64, &payload, MAX_DATA_URI_ASSET_BYTES)?;
        let mut file_name = data_asset_file_name(asset, media_type.as_deref(), index);
        let mut collision = 1u32;
        let relative = loop {
            let relative = PathBuf::from("assets")
                .join(kind_type(kind))
                .join("3d")
                .join(&file_name);
            if !target.join(&relative).exists() {
                break relative;
            }
            file_name = disambiguated_name(&file_name, collision);
            collision += 1;
        };
        write_import_entry(target, &relative, &bytes)?;
        asset.uri = format!("embeded://{}", relative.to_string_lossy());
        changed = true;
    }
    if changed {
        write_card_json(card, &target.join("character.json"))?;
    }
    Ok(())
}

fn kind_type(kind: crate::character_assets::EneAssetKind) -> &'static str {
    match kind {
        crate::character_assets::EneAssetKind::Vrm => "x_vrm",
        crate::character_assets::EneAssetKind::Vrma => "x_vrma",
    }
}

fn data_asset_file_name(asset: &CharacterAsset, media_type: Option<&str>, index: usize) -> String {
    let base = sanitize_name(&asset.name).unwrap_or_else(|| format!("asset_{index}"));
    let ext = sanitize_extension(&asset.ext)
        .or_else(|| media_type.and_then(extension_from_media_type))
        .unwrap_or_else(|| "bin".to_string());
    format!("{base}.{ext}")
}

fn disambiguated_name(file_name: &str, counter: u32) -> String {
    let path = Path::new(file_name);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    if extension.is_empty() {
        format!("{stem}_{counter}")
    } else {
        format!("{stem}_{counter}.{extension}")
    }
}

fn sanitize_extension(ext: &str) -> Option<String> {
    let lower = ext.trim().to_ascii_lowercase();
    if lower.is_empty() || lower.len() > 8 || !lower.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(lower)
}

fn extension_from_media_type(media_type: &str) -> Option<String> {
    let suffix = media_type.split(';').next()?.split('/').next_back()?;
    sanitize_extension(suffix)
}

fn import_folder_name(card: &CharacterCardV3, src: &Path) -> Result<String, EneConfigError> {
    if let Some(name) = sanitize_name(card.data.get_character_name()) {
        return Ok(name);
    }
    if let Some(stem) = src
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(sanitize_name)
    {
        return Ok(stem);
    }
    Err(EneConfigError::CharacterImportUnnamed)
}

/// Keeps Unicode letters/digits plus `-_.`, maps everything else to `_`, and
/// rejects names that would collapse to `.`/`..`/empty.
fn sanitize_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let out: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .chars()
        .take(MAX_IMPORT_NAME_CHARS)
        .collect();
    if out.is_empty() || out == "." || out == ".." {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use base64::Engine as _;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use zip::write::SimpleFileOptions;

    use super::*;

    const CARD_JSON: &str =
        r#"{"spec":"chara_card_v3","spec_version":"3.0","data":{"name":"Ada"}}"#;

    fn base64(json: &str) -> String {
        STANDARD.encode(json.as_bytes())
    }

    fn png(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PNG_SIGNATURE);
        out.extend_from_slice(&[0, 0, 0, 13]);
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&[0; 13]);
        out.extend_from_slice(&[0; 4]);
        for chunk in chunks {
            out.extend_from_slice(chunk);
        }
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(b"IEND");
        out.extend_from_slice(&[0; 4]);
        out
    }

    fn text_chunk(keyword: &str, payload: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::new();
        let len = keyword.len() + 1 + payload.len();
        chunk.extend_from_slice(&(len as u32).to_be_bytes());
        chunk.extend_from_slice(b"tEXt");
        chunk.extend_from_slice(keyword.as_bytes());
        chunk.push(0);
        chunk.extend_from_slice(payload);
        chunk.extend_from_slice(&[0; 4]);
        chunk
    }

    fn ztext_chunk(keyword: &str, payload: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("deflate text");
        let compressed = encoder.finish().expect("finish deflate");
        let mut chunk = Vec::new();
        let len = keyword.len() + 1 + 1 + compressed.len();
        chunk.extend_from_slice(&(len as u32).to_be_bytes());
        chunk.extend_from_slice(b"zTXt");
        chunk.extend_from_slice(keyword.as_bytes());
        chunk.push(0);
        chunk.push(0);
        chunk.extend_from_slice(&compressed);
        chunk.extend_from_slice(&[0; 4]);
        chunk
    }

    fn itext_chunk(keyword: &str, payload: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("deflate text");
        let compressed = encoder.finish().expect("finish deflate");
        let mut chunk = Vec::new();
        let len = keyword.len() + 7 + compressed.len();
        chunk.extend_from_slice(&(len as u32).to_be_bytes());
        chunk.extend_from_slice(b"iTXt");
        chunk.extend_from_slice(keyword.as_bytes());
        chunk.push(0);
        chunk.push(1);
        chunk.push(0);
        chunk.extend_from_slice(b"en");
        chunk.push(0);
        chunk.push(0);
        chunk.extend_from_slice(&compressed);
        chunk.extend_from_slice(&[0; 4]);
        chunk
    }

    fn charx(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in entries {
                if content.is_empty() && name.ends_with('/') {
                    writer
                        .add_directory(*name, options)
                        .expect("add zip directory");
                } else {
                    writer.start_file(*name, options).expect("start zip entry");
                    writer.write_all(content).expect("write zip entry");
                }
            }
            writer.finish().expect("finish zip");
        }
        buf
    }

    /// Minimal stored zip with one entry per tuple. `external_attrs` is the
    /// central-directory unix mode shifted into the high bits and `flags`
    /// the general-purpose bitfield (bit 0 = encrypted); this exercises
    /// entry metadata that [`ZipWriter`] cannot produce.
    fn raw_zip(entries: &[(&str, &[u8], u32, u32, u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, content, claimed_size, external_attrs, flags) in entries {
            let name_bytes = name.as_bytes();
            let local_offset = out.len() as u32;
            let mut crc = flate2::Crc::new();
            crc.update(content);
            let crc_sum = crc.sum();
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&crc_sum.to_le_bytes());
            out.extend_from_slice(&claimed_size.to_le_bytes());
            out.extend_from_slice(&claimed_size.to_le_bytes());
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(content);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&0x031eu16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&flags.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&crc_sum.to_le_bytes());
            central.extend_from_slice(&claimed_size.to_le_bytes());
            central.extend_from_slice(&claimed_size.to_le_bytes());
            central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&external_attrs.to_le_bytes());
            central.extend_from_slice(&local_offset.to_le_bytes());
            central.extend_from_slice(name_bytes);
        }
        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn png_ztext_raw_deflate_is_tolerated() {
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(base64(CARD_JSON).as_bytes())
            .expect("deflate text");
        let compressed = encoder.finish().expect("finish deflate");
        let mut chunk = Vec::new();
        let len = "ccv3".len() + 1 + 1 + compressed.len();
        chunk.extend_from_slice(&(len as u32).to_be_bytes());
        chunk.extend_from_slice(b"zTXt");
        chunk.extend_from_slice(b"ccv3");
        chunk.push(0);
        chunk.push(0);
        chunk.extend_from_slice(&compressed);
        chunk.extend_from_slice(&[0; 4]);

        let card = load_card_from_bytes(&png(&[chunk])).expect("raw-deflate card loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn oversized_card_file_is_rejected_before_reading() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("big.png");
        let file = std::fs::File::create(&src).expect("create sparse file");
        file.set_len(MAX_CARD_FILE_BYTES + 1).expect("extend file");
        drop(file);

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CardFileTooLarge(_))
        ));
        assert!(matches!(
            load_card_from_path(&src),
            Err(EneConfigError::CardFileTooLarge(_))
        ));
    }

    #[test]
    fn import_charx_rejects_symlink_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("evil.charx");
        let bytes = raw_zip(&[
            (
                "card.json",
                CARD_JSON.as_bytes(),
                CARD_JSON.len() as u32,
                0o100_644 << 16,
                0,
            ),
            ("link", b"", 0, 0o120_777 << 16, 0),
        ]);
        std::fs::write(&src, bytes).expect("write charx");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharxUnsafePath(_))
        ));
        assert!(
            !assets.join("characters/Ada").exists(),
            "failed import must leave no partial folder"
        );
    }

    #[test]
    fn import_charx_rejects_encrypted_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("evil.charx");
        let bytes = raw_zip(&[(
            "card.json",
            CARD_JSON.as_bytes(),
            CARD_JSON.len() as u32,
            0o100_644 << 16,
            1,
        )]);
        std::fs::write(&src, bytes).expect("write charx");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharxEncrypted(_))
        ));
    }

    #[test]
    fn import_charx_rejects_total_size_cap() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("huge.charx");
        let one_gib = MAX_CHARX_ENTRY_BYTES as u32;
        let entries = [
            (
                "card.json",
                CARD_JSON.as_bytes(),
                CARD_JSON.len() as u32,
                0o100_644 << 16,
                0,
            ),
            ("a.bin", b"", one_gib, 0o100_644 << 16, 0),
            ("b.bin", b"", one_gib, 0o100_644 << 16, 0),
            ("c.bin", b"", one_gib, 0o100_644 << 16, 0),
            ("d.bin", b"", one_gib, 0o100_644 << 16, 0),
            ("e.bin", b"", one_gib, 0o100_644 << 16, 0),
        ];
        std::fs::write(&src, raw_zip(&entries)).expect("write charx");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharxTooLarge(_))
        ));
    }

    #[test]
    fn data_url_assets_with_same_name_are_disambiguated() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.png");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "assets":[
                    {"type":"x_vrm","uri":"data:model/vrm;base64,QUJD","name":"Model","ext":"vrm"},
                    {"type":"x_vrm","uri":"data:model/vrm;base64,REVG","name":"Model","ext":"vrm"}
                ]
            }
        }"#;
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(card_json).as_bytes())]),
        )
        .expect("write png");

        import_character_file_in(&src, &assets).expect("imports");
        assert_eq!(
            std::fs::read(assets.join("characters/Ada/assets/x_vrm/3d/Model.vrm"))
                .expect("first asset"),
            b"ABC"
        );
        assert_eq!(
            std::fs::read(assets.join("characters/Ada/assets/x_vrm/3d/Model_1.vrm"))
                .expect("second asset"),
            b"DEF"
        );
        let card = load_card_from_path(&assets.join("characters/Ada/character.json"))
            .expect("card readable");
        assert_eq!(
            card.data.assets[0].uri,
            "embeded://assets/x_vrm/3d/Model.vrm"
        );
        assert_eq!(
            card.data.assets[1].uri,
            "embeded://assets/x_vrm/3d/Model_1.vrm"
        );
    }

    #[test]
    fn import_charx_skips_directory_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.charx");
        let bytes = charx(&[
            ("card.json", CARD_JSON.as_bytes()),
            ("assets/", b""),
            ("assets/x_vrm/3d/model.vrm", b"vrm bytes"),
        ]);
        std::fs::write(&src, bytes).expect("write charx");

        import_character_file_in(&src, &assets).expect("charx imports");
        assert!(
            assets
                .join("characters/Ada/assets/x_vrm/3d/model.vrm")
                .exists(),
            "file under directory entries extracts"
        );
    }

    #[test]
    fn png_ccv3_text_chunk_loads() {
        let bytes = png(&[text_chunk("ccv3", base64(CARD_JSON).as_bytes())]);
        let card = load_card_from_bytes(&bytes).expect("card loads from png");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn png_ccv3_raw_json_is_tolerated() {
        let bytes = png(&[text_chunk("ccv3", CARD_JSON.as_bytes())]);
        let card = load_card_from_bytes(&bytes).expect("raw json card loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn png_ztext_and_itext_chunks_load() {
        let ztext = png(&[ztext_chunk("ccv3", base64(CARD_JSON).as_bytes())]);
        let card = load_card_from_bytes(&ztext).expect("zTXt card loads");
        assert_eq!(card.data.name, "Ada");

        let itext = png(&[itext_chunk("ccv3", base64(CARD_JSON).as_bytes())]);
        let card = load_card_from_bytes(&itext).expect("iTXt card loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn png_chara_v2_chunk_wraps_to_v3() {
        let v2 = r#"{"name":"V2","description":"legacy"}"#;
        let bytes = png(&[text_chunk("chara", base64(v2).as_bytes())]);
        let card = load_card_from_bytes(&bytes).expect("V2 card loads");
        assert_eq!(card.spec, "chara_card_v3");
        assert_eq!(card.data.name, "V2");
        assert_eq!(card.data.description, "legacy");
    }

    #[test]
    fn png_ccv3_takes_precedence_over_chara() {
        let v2 = r#"{"name":"V2"}"#;
        let bytes = png(&[
            text_chunk("chara", base64(v2).as_bytes()),
            text_chunk("ccv3", base64(CARD_JSON).as_bytes()),
        ]);
        let card = load_card_from_bytes(&bytes).expect("card loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn png_without_card_chunk_errors() {
        let bytes = png(&[text_chunk("Comment", b"no card here")]);
        assert!(matches!(
            load_card_from_bytes(&bytes),
            Err(EneConfigError::PngCardMissingChunk)
        ));
    }

    #[test]
    fn png_with_bad_signature_errors() {
        assert!(matches!(
            load_card_from_bytes(b"not a png"),
            Err(EneConfigError::JsonError(_))
        ));
    }

    #[test]
    fn charx_card_loads_from_zip() {
        let bytes = charx(&[("card.json", CARD_JSON.as_bytes())]);
        let card = load_card_from_bytes(&bytes).expect("charx card loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn charx_without_card_json_errors() {
        let bytes = charx(&[("other.json", b"{}")]);
        assert!(matches!(
            load_card_from_bytes(&bytes),
            Err(EneConfigError::CharxMissingCard)
        ));
    }

    #[test]
    fn json_bytes_load_directly() {
        let card = load_card_from_bytes(CARD_JSON.as_bytes()).expect("json loads");
        assert_eq!(card.data.name, "Ada");
    }

    #[test]
    fn import_png_materializes_folder() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.png");
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(CARD_JSON).as_bytes())]),
        )
        .expect("write png");

        let imported = import_character_file_in(&src, &assets).expect("png imports");
        assert_eq!(imported.folder, "Ada");
        assert_eq!(imported.card_path, "characters/Ada/character.json");
        let card = load_card_from_path(&assets.join(&imported.card_path)).expect("card readable");
        assert_eq!(card.data.name, "Ada");
        assert!(assets.join("characters/Ada/avatar.png").exists());
    }

    #[test]
    fn import_png_v2_card_uses_v2_name() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("legacy.png");
        let v2 = r#"{"name":"Legacy"}"#;
        std::fs::write(&src, png(&[text_chunk("chara", base64(v2).as_bytes())]))
            .expect("write png");

        let imported = import_character_file_in(&src, &assets).expect("png imports");
        assert_eq!(imported.name, "Legacy");
        assert!(assets.join("characters/Legacy/character.json").exists());
    }

    #[test]
    fn import_rejects_existing_target() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(assets.join("characters/Ada")).expect("create target");
        let src = tmp.path().join("ada.png");
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(CARD_JSON).as_bytes())]),
        )
        .expect("write png");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharacterImportExists(_))
        ));
    }

    #[test]
    fn import_charx_extracts_assets() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.charx");
        let vrm = b"vrm bytes";
        let bytes = charx(&[
            ("card.json", CARD_JSON.as_bytes()),
            ("assets/x_vrm/3d/model.vrm", vrm),
        ]);
        std::fs::write(&src, bytes).expect("write charx");

        let imported = import_character_file_in(&src, &assets).expect("charx imports");
        assert_eq!(imported.folder, "Ada");
        assert_eq!(
            std::fs::read(assets.join("characters/Ada/assets/x_vrm/3d/model.vrm"))
                .expect("vrm extracted"),
            vrm
        );
        assert!(
            !assets.join("characters/Ada/card.json").exists(),
            "card.json must be renamed to character.json"
        );
        assert!(assets.join("characters/Ada/character.json").exists());
    }

    #[test]
    fn import_charx_rejects_traversal_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("evil.charx");
        let bytes = charx(&[("card.json", CARD_JSON.as_bytes()), ("../evil", b"x")]);
        std::fs::write(&src, bytes).expect("write charx");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharxUnsafePath(_))
        ));
        assert!(
            !assets.join("characters/Ada").exists(),
            "failed import must leave no partial folder"
        );
    }

    #[test]
    fn import_charx_rejects_absolute_entries() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("evil.charx");
        let bytes = charx(&[("card.json", CARD_JSON.as_bytes()), ("/etc/passwd", b"x")]);
        std::fs::write(&src, bytes).expect("write charx");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::CharxUnsafePath(_))
        ));
    }

    #[test]
    fn import_json_file_is_rejected_with_guidance() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("card.json");
        std::fs::write(&src, CARD_JSON).expect("write json");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::UnsupportedCardFile(_))
        ));
    }

    #[test]
    fn import_materializes_data_url_assets() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.png");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "assets":[{
                    "type":"x_vrm",
                    "uri":"data:model/vrm;base64,QUJD",
                    "name":"Model",
                    "ext":"vrm"
                }]
            }
        }"#;
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(card_json).as_bytes())]),
        )
        .expect("write png");

        import_character_file_in(&src, &assets).expect("imports");
        let materialized = std::fs::read(assets.join("characters/Ada/assets/x_vrm/3d/Model.vrm"))
            .expect("data url materialized");
        assert_eq!(materialized, b"ABC");
        let card = load_card_from_path(&assets.join("characters/Ada/character.json"))
            .expect("card readable");
        assert_eq!(
            card.data.assets[0].uri,
            "embeded://assets/x_vrm/3d/Model.vrm"
        );
    }

    #[test]
    fn import_rejects_unsafe_asset_uris() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("evil.png");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Evil",
                "assets":[{
                    "type":"x_vrm",
                    "uri":"embeded://../../secret.vrm",
                    "name":"Model",
                    "ext":"vrm"
                }]
            }
        }"#;
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(card_json).as_bytes())]),
        )
        .expect("write png");

        assert!(matches!(
            import_character_file_in(&src, &assets),
            Err(EneConfigError::UnsafeAssetPath(_))
        ));
    }

    #[test]
    fn folder_names_are_sanitized() {
        assert_eq!(sanitize_name("Ada Lovelace").expect("name"), "Ada_Lovelace");
        assert_eq!(sanitize_name("アリス").expect("cjk name"), "アリス");
        assert_eq!(sanitize_name(".."), None);
        assert_eq!(sanitize_name("..."), None);
        assert_eq!(sanitize_name("  "), None);
        assert!(
            sanitize_name("a".repeat(100).as_str())
                .expect("long name")
                .chars()
                .count()
                <= MAX_IMPORT_NAME_CHARS
        );
    }

    const LOCALIZED_BASE_JSON: &str = r#"{
        "spec":"chara_card_v3",
        "spec_version":"3.0",
        "data":{
            "name":"Ada",
            "description":"Base description",
            "personality":"Base personality",
            "first_mes":"Hello!",
            "alternate_greetings":["Hi"],
            "nickname":"Ada",
            "tags":["engineer"],
            "character_book":{
                "entries":[
                    {
                        "id":"lore-1",
                        "keys":["cat","kitty"],
                        "secondary_keys":["pet"],
                        "content":"Base lore",
                        "enabled":true,
                        "insertion_order":0,
                        "use_regex":false
                    }
                ]
            }
        }
    }"#;

    const JA_DIFF_JSON: &str = r#"{
        "description":"日本語の説明",
        "first_mes":"やっほー！",
        "alternate_greetings":["こんにちは"],
        "nickname":"エイダ",
        "tags":["エンジニア"],
        "character_book":{
            "entries":[
                {
                    "id":"lore-1",
                    "keys":["猫","ねこ"],
                    "secondary_keys":["ペット"],
                    "content":"日本語のロア"
                }
            ]
        }
    }"#;

    fn assert_ja_applied(card: &CharacterCardV3) {
        assert_eq!(card.data.description, "日本語の説明");
        assert_eq!(card.data.first_mes, "やっほー！");
        assert_eq!(card.data.alternate_greetings, ["こんにちは"]);
        assert_eq!(card.data.nickname, "エイダ");
        assert_eq!(card.data.tags, ["エンジニア"]);
        assert_eq!(card.data.personality, "Base personality");
        let entry = &card
            .data
            .character_book
            .as_ref()
            .expect("book present")
            .entries[0];
        assert_eq!(entry.keys, ["猫", "ねこ"]);
        assert_eq!(entry.secondary_keys, Some(vec!["ペット".to_string()]));
        assert_eq!(entry.content, "日本語のロア");
    }

    #[test]
    fn localized_folder_sidecar_layers_over_base() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("character.json"), LOCALIZED_BASE_JSON).expect("write base");
        std::fs::write(dir.join("character.ja.json"), JA_DIFF_JSON).expect("write diff");

        let card = load_card_from_path_localized(&dir.join("character.json"), "ja").expect("loads");
        assert_ja_applied(&card);

        let base = load_card_from_path(&dir.join("character.json")).expect("base loads");
        assert_eq!(base.data.description, "Base description");
        assert_eq!(
            base.data.character_book.expect("book").entries[0].keys,
            ["cat", "kitty"]
        );
    }

    #[test]
    fn localized_load_aliases_locale_codes_to_sidecar_names() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("character.json"), LOCALIZED_BASE_JSON).expect("write base");
        std::fs::write(dir.join("character.ja.json"), JA_DIFF_JSON).expect("write diff");

        for code in ["ja-JP", "JA", "jp"] {
            let card = load_card_from_path_localized(&dir.join("character.json"), code)
                .expect("alias resolves");
            assert_eq!(card.data.description, "日本語の説明", "code {code}");
        }
    }

    #[test]
    fn localized_load_without_sidecar_returns_base() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("character.json"), LOCALIZED_BASE_JSON).expect("write base");

        let card = load_card_from_path_localized(&dir.join("character.json"), "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn malformed_sidecar_falls_back_to_base() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("character.json"), LOCALIZED_BASE_JSON).expect("write base");
        std::fs::write(dir.join("character.ja.json"), "{not json").expect("write diff");

        let card = load_card_from_path_localized(&dir.join("character.json"), "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn localized_charx_reads_root_diff_entry() {
        let bytes = charx(&[
            ("card.json", LOCALIZED_BASE_JSON.as_bytes()),
            ("character.ja.json", JA_DIFF_JSON.as_bytes()),
        ]);

        let card = load_card_from_bytes_localized(&bytes, "ja").expect("loads");
        assert_ja_applied(&card);

        let base = load_card_from_bytes(&bytes).expect("base loads");
        assert_eq!(base.data.description, "Base description");
    }

    #[test]
    fn localized_png_reads_embedded_locales() {
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "personality":"Base personality",
                "character_book":{
                    "entries":[
                        {
                            "id":"lore-1",
                            "keys":["cat"],
                            "content":"Base lore",
                            "enabled":true,
                            "insertion_order":0,
                            "use_regex":false
                        }
                    ]
                },
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{
                                "description":"日本語の説明",
                                "character_book":{
                                    "entries":[
                                        {"id":"lore-1","keys":["猫"],"content":"日本語のロア"}
                                    ]
                                }
                            }
                        }
                    }
                }
            }
        }"#;
        let bytes = png(&[text_chunk("ccv3", base64(card_json).as_bytes())]);

        let card = load_card_from_bytes_localized(&bytes, "ja").expect("loads");
        assert_eq!(card.data.description, "日本語の説明");
        let entry = &card
            .data
            .character_book
            .as_ref()
            .expect("book present")
            .entries[0];
        assert_eq!(entry.keys, ["猫"]);
        assert_eq!(entry.content, "日本語のロア");
        assert!(
            card.data
                .extensions
                .ene
                .as_ref()
                .is_none_or(|ene| ene.locales.is_none()),
            "locale bag is stripped after merging"
        );
    }

    #[test]
    fn localized_json_uses_embedded_locales() {
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{"description":"日本語の説明"}
                        }
                    }
                }
            }
        }"#;

        let card = load_card_from_bytes_localized(card_json.as_bytes(), "ja").expect("loads");
        assert_eq!(card.data.description, "日本語の説明");
    }

    #[test]
    fn import_png_materializes_embedded_locales_to_sidecars() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.png");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{"description":"日本語の説明"}
                        }
                    }
                }
            }
        }"#;
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(card_json).as_bytes())]),
        )
        .expect("write png");

        import_character_file_in(&src, &assets).expect("imports");
        let folder = assets.join("characters/Ada");
        assert!(folder.join("character.ja.json").exists(), "sidecar written");
        let base = load_card_from_path(&folder.join("character.json")).expect("card readable");
        assert!(
            base.data
                .extensions
                .ene
                .as_ref()
                .is_none_or(|ene| ene.locales.is_none()),
            "character.json no longer embeds the locale bag"
        );
        let localized =
            load_card_from_path_localized(&folder.join("character.json"), "ja").expect("loads");
        assert_eq!(localized.data.description, "日本語の説明");
    }

    #[test]
    fn import_charx_keeps_zip_sidecar_and_materializes_embedded_only_locales() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.charx");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{"description":"埋め込み側の日本語"},
                            "fr":{"description":"Description française"}
                        }
                    }
                }
            }
        }"#;
        let bytes = charx(&[
            ("card.json", card_json.as_bytes()),
            ("character.ja.json", JA_DIFF_JSON.as_bytes()),
        ]);
        std::fs::write(&src, bytes).expect("write charx");

        import_character_file_in(&src, &assets).expect("imports");
        let folder = assets.join("characters/Ada");
        let zip_sidecar =
            std::fs::read_to_string(folder.join("character.ja.json")).expect("zip sidecar kept");
        assert!(
            zip_sidecar.contains("日本語の説明"),
            "zip-provided sidecar wins over the embedded bag"
        );
        let fr_sidecar = std::fs::read_to_string(folder.join("character.fr.json"))
            .expect("embedded-only locale materialized");
        assert!(fr_sidecar.contains("Description française"));
        let base = load_card_from_path(&folder.join("character.json")).expect("card readable");
        assert!(
            base.data
                .extensions
                .ene
                .as_ref()
                .is_none_or(|ene| ene.locales.is_none()),
            "character.json no longer embeds the locale bag"
        );
    }

    #[test]
    fn charx_localized_encrypted_diff_entry_errors() {
        let bytes = raw_zip(&[
            (
                "card.json",
                LOCALIZED_BASE_JSON.as_bytes(),
                LOCALIZED_BASE_JSON.len() as u32,
                0,
                0,
            ),
            (
                "character.ja.json",
                JA_DIFF_JSON.as_bytes(),
                JA_DIFF_JSON.len() as u32,
                0,
                1,
            ),
        ]);

        assert!(matches!(
            load_card_from_bytes_localized(&bytes, "ja"),
            Err(EneConfigError::CharxEncrypted(name)) if name == "character.ja.json"
        ));
        let base = load_card_from_bytes(&bytes).expect("base load is unaffected");
        assert_eq!(base.data.description, "Base description");
    }

    #[test]
    fn charx_localized_oversized_diff_entry_falls_back_to_base() {
        let bytes = raw_zip(&[
            (
                "card.json",
                LOCALIZED_BASE_JSON.as_bytes(),
                LOCALIZED_BASE_JSON.len() as u32,
                0,
                0,
            ),
            (
                "character.ja.json",
                JA_DIFF_JSON.as_bytes(),
                (MAX_CHARX_ENTRY_BYTES + 1) as u32,
                0,
                0,
            ),
        ]);

        let card = load_card_from_bytes_localized(&bytes, "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn charx_localized_malformed_diff_entry_falls_back_to_base() {
        let bytes = charx(&[
            ("card.json", LOCALIZED_BASE_JSON.as_bytes()),
            ("character.ja.json", b"{not json"),
        ]);

        let card = load_card_from_bytes_localized(&bytes, "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn localized_png_canonicalizes_embedded_locale_keys() {
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja-JP":{"description":"日本語の説明"}
                        }
                    }
                }
            }
        }"#;
        let bytes = png(&[text_chunk("ccv3", base64(card_json).as_bytes())]);

        let card = load_card_from_bytes_localized(&bytes, "ja").expect("loads");
        assert_eq!(card.data.description, "日本語の説明");
    }

    #[test]
    fn unknown_diff_fields_make_the_whole_diff_skip() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("character.json"), LOCALIZED_BASE_JSON).expect("write base");
        std::fs::write(
            dir.join("character.ja.json"),
            r#"{"first_mess":"タイポしたフィールド"}"#,
        )
        .expect("write diff");

        let card = load_card_from_path_localized(&dir.join("character.json"), "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
        assert_eq!(card.data.first_mes, "Hello!");
    }

    #[test]
    fn localized_load_ignores_sidecars_for_non_standard_card_names() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join("Ada");
        std::fs::create_dir_all(&dir).expect("create card dir");
        std::fs::write(dir.join("exported.json"), LOCALIZED_BASE_JSON).expect("write base");
        std::fs::write(dir.join("character.ja.json"), JA_DIFF_JSON).expect("write diff");

        let card = load_card_from_path_localized(&dir.join("exported.json"), "ja").expect("loads");
        assert_eq!(card.data.description, "Base description");
    }

    #[test]
    fn import_charx_overwrites_malformed_sidecar_with_embedded_locale() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let src = tmp.path().join("ada.charx");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{"description":"埋め込みの日本語"}
                        }
                    }
                }
            }
        }"#;
        let bytes = charx(&[
            ("card.json", card_json.as_bytes()),
            ("character.ja.json", b"{not json"),
        ]);
        std::fs::write(&src, bytes).expect("write charx");

        import_character_file_in(&src, &assets).expect("imports");
        let folder = assets.join("characters/Ada");
        let sidecar =
            std::fs::read_to_string(folder.join("character.ja.json")).expect("sidecar rewritten");
        assert!(
            sidecar.contains("埋め込みの日本語"),
            "malformed zip sidecar is replaced by the embedded locale"
        );
    }

    #[test]
    fn export_character_card_merges_charx_root_diff() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let src = tmp.path().join("ada.charx");
        let bytes = charx(&[
            ("card.json", LOCALIZED_BASE_JSON.as_bytes()),
            ("character.ja.json", JA_DIFF_JSON.as_bytes()),
        ]);
        std::fs::write(&src, bytes).expect("write charx");
        let out = tmp.path().join("ada-ja.json");

        crate::export_character_card(&src.to_string_lossy(), "ja", &out).expect("exports");

        let exported: CharacterCardV3 =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("read export"))
                .expect("export parses as CCv3");
        assert_ja_applied(&exported);
    }

    #[test]
    fn export_character_card_merges_png_embedded_locales() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let src = tmp.path().join("ada.png");
        let card_json = r#"{
            "spec":"chara_card_v3",
            "spec_version":"3.0",
            "data":{
                "name":"Ada",
                "description":"Base description",
                "personality":"Base personality",
                "extensions":{
                    "ene":{
                        "locales":{
                            "ja":{"description":"日本語の説明"}
                        }
                    }
                }
            }
        }"#;
        std::fs::write(
            &src,
            png(&[text_chunk("ccv3", base64(card_json).as_bytes())]),
        )
        .expect("write png");
        let out = tmp.path().join("ada-ja.json");

        crate::export_character_card(&src.to_string_lossy(), "ja", &out).expect("exports");

        let exported: CharacterCardV3 =
            serde_json::from_str(&std::fs::read_to_string(&out).expect("read export"))
                .expect("export parses as CCv3");
        assert_eq!(exported.data.description, "日本語の説明");
        assert_eq!(exported.data.personality, "Base personality");
    }
}
