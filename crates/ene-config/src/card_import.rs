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
    let bytes = std::fs::read(src).map_err(EneConfigError::CardReadError)?;
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
    let bytes = std::fs::read(path).map_err(EneConfigError::CardReadError)?;
    load_card_from_bytes(&bytes)
}

/// Parses card bytes by magic: PNG signature, zip signature, else JSON.
pub(crate) fn load_card_from_bytes(bytes: &[u8]) -> Result<CharacterCardV3, EneConfigError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        return serde_json::from_value(png_card_json(bytes)?).map_err(EneConfigError::JsonError);
    }
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        return serde_json::from_value(charx_card_json(bytes)?).map_err(EneConfigError::JsonError);
    }
    serde_json::from_slice(bytes).map_err(EneConfigError::JsonError)
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
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(data)
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
    let mut archive =
        ZipArchive::new(std::io::Cursor::new(bytes)).map_err(EneConfigError::CharxError)?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(EneConfigError::CharxError)?;
        if file.name() != "card.json" {
            continue;
        }
        if file.encrypted() {
            return Err(EneConfigError::CharxEncrypted("card.json".to_string()));
        }
        let mut content = Vec::new();
        file.by_ref()
            .take(MAX_CHARX_ENTRY_BYTES + 1)
            .read_to_end(&mut content)
            .map_err(|e| EneConfigError::CharxError(zip::result::ZipError::Io(e)))?;
        if content.len() as u64 > MAX_CHARX_ENTRY_BYTES {
            return Err(EneConfigError::CharxTooLarge("card.json".to_string()));
        }
        return serde_json::from_slice(&content).map_err(EneConfigError::JsonError);
    }
    Err(EneConfigError::CharxMissingCard)
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
    for (index, component) in name.split('/').enumerate() {
        if component.is_empty() || component == "." || component == ".." {
            return Err(EneConfigError::CharxUnsafePath(name.to_string()));
        }
        if index == 0 && is_drive_prefix(component) {
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
    ensure_import_target(&target, &folder)?;
    materialize_data_assets(&mut card, &target)?;
    write_card_json(&card, &target.join("character.json"))?;
    std::fs::write(target.join("avatar.png"), bytes).map_err(EneConfigError::IoError)?;
    Ok(ImportedCharacter {
        name: card.data.get_character_name().to_string(),
        card_path: format!("characters/{folder}/character.json"),
        folder,
    })
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
    ensure_import_target(&target, &folder)?;

    let mut archive =
        ZipArchive::new(std::io::Cursor::new(bytes)).map_err(EneConfigError::CharxError)?;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
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
        write_import_entry(&target, relative, &content)?;
    }

    materialize_data_assets(&mut card, &target)?;
    write_card_json(&card, &target.join("character.json"))?;
    Ok(ImportedCharacter {
        name: card.data.get_character_name().to_string(),
        card_path: format!("characters/{folder}/character.json"),
        folder,
    })
}

fn ensure_import_target(target: &Path, folder: &str) -> Result<(), EneConfigError> {
    if target.exists() {
        return Err(EneConfigError::CharacterImportExists(folder.to_string()));
    }
    std::fs::create_dir_all(target).map_err(EneConfigError::IoError)
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
        if asset.ene_kind().is_none() {
            continue;
        }
        match resolve_asset_uri(&asset.uri) {
            Ok(_) => {}
            Err(EneConfigError::UnsupportedAssetUriScheme(scheme)) => {
                tracing::warn!(
                    scheme = %scheme,
                    asset = %asset.name,
                    "Skipping asset with unsupported URI scheme"
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
        let file_name = data_asset_file_name(asset, media_type.as_deref(), index);
        let relative = PathBuf::from("assets")
            .join(kind_type(kind))
            .join("3d")
            .join(&file_name);
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
    use flate2::write::DeflateEncoder;
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
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
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
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
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
}
