use crate::affect::AffectBaseline;
use crate::error::CompanionError;
use crate::soul::NewSoul;
use crate::store::CompanionStore;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;
use zip::write::SimpleFileOptions;

const FORMAT_VERSION: u32 = 1;
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Result of installing a package into `<data>/characters/<id>@<version>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub id: String,
    pub version: String,
    pub kind: PackageKind,
    pub path: PathBuf,
    pub digest: String,
    pub origin_unverified: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Character,
    Soul,
    Body,
}

impl PackageKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Soul => "soul",
            Self::Body => "body",
        }
    }

    fn parse(raw: &str) -> Result<Self, CompanionError> {
        match raw {
            "character" => Ok(Self::Character),
            "soul" => Ok(Self::Soul),
            "body" => Ok(Self::Body),
            other => Err(CompanionError::package(format!("unknown kind {other}"))),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ManifestFile {
    package: PackageMeta,
    contents: Option<ContentsMeta>,
    integrity: Option<IntegrityMeta>,
    license: Option<LicenseMeta>,
    #[serde(default)]
    locales: BTreeMap<String, LocaleFields>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct LocaleFields {
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PackageMeta {
    kind: String,
    id: String,
    version: String,
    format_version: u32,
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[expect(
    dead_code,
    reason = "parsed for validation; fields reserved for export"
)]
struct ContentsMeta {
    #[serde(default)]
    soul: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct IntegrityMeta {
    #[serde(default)]
    digest: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[expect(
    dead_code,
    reason = "parsed for validation; fields reserved for export"
)]
struct LicenseMeta {
    #[serde(default)]
    spdx: String,
    #[serde(default)]
    redistribute: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SoulFile {
    identity: Option<IdentityFile>,
    affect: Option<AffectFile>,
    voice: Option<VoiceFile>,
    skills: Option<SkillsFile>,
    proactive: Option<ProactiveFile>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct IdentityFile {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AffectFile {
    baseline: Option<AffectBaseline>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct VoiceFile {
    #[serde(default)]
    voice_ref: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SkillsFile {
    #[serde(default)]
    refs: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ProactiveFile {
    #[serde(default)]
    tendency: String,
}

/// ZIP local-file magic (`PK\x03\x04`) or empty-archive magic (`PK\x05\x06`).
#[must_use]
pub fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]) || bytes.starts_with(&[0x50, 0x4b, 0x05, 0x06])
}

/// True when `bytes` is a zip that contains `manifest.toml` (Ene package).
#[must_use]
pub fn looks_like_package_zip(bytes: &[u8]) -> bool {
    if !looks_like_zip(bytes) {
        return false;
    }
    ZipArchive::new(Cursor::new(bytes))
        .is_ok_and(|mut archive| archive.by_name("manifest.toml").is_ok())
}

/// Avatar file inside an installed package (`body.toml` `avatar`, else first `.vrm`).
#[must_use]
pub fn avatar_path_for_install(dir: &Path) -> Option<PathBuf> {
    let body_dir = dir.join("body");
    let toml_path = body_dir.join("body.toml");
    if let Ok(text) = fs::read_to_string(&toml_path)
        && let Some(rel) = parse_avatar_field(&text)
    {
        let candidates = [body_dir.join(&rel), dir.join(&rel)];
        if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
            return Some(path);
        }
    }
    let avatar_dir = body_dir.join("avatar");
    let Ok(entries) = fs::read_dir(&avatar_dir) else {
        return None;
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_vrm = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vrm"));
        if is_vrm {
            found.push(path);
        }
    }
    found.sort();
    found.into_iter().next()
}

fn parse_avatar_field(toml_text: &str) -> Option<String> {
    for line in toml_text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("avatar") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let inner = rest.trim_start_matches(quote).trim_end_matches(quote);
        if !inner.is_empty() {
            return Some(inner.to_owned());
        }
    }
    None
}

/// Build a `.enechar` / `.enesoul` / `.enebody` zip from in-memory files.
pub fn pack_archive(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, CompanionError> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            writer
                .start_file(name, options)
                .map_err(|err| CompanionError::package(err.to_string()))?;
            writer
                .write_all(bytes)
                .map_err(|err| CompanionError::package(err.to_string()))?;
        }
        writer
            .finish()
            .map_err(|err| CompanionError::package(err.to_string()))?;
    }
    Ok(buf)
}

/// SHA-256 over sorted paths+bytes, skipping `manifest.toml`'s digest line.
#[must_use]
pub fn content_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.as_bytes());
        if path == "manifest.toml" {
            let stripped = strip_digest_line(bytes);
            hasher.update(
                u64::try_from(stripped.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            hasher.update(stripped.as_bytes());
        } else {
            hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
            hasher.update(bytes);
        }
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn strip_digest_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim_start().starts_with("digest"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Install an archive into `home/<id>@<version>/`.
pub fn install_archive(
    store: &CompanionStore,
    home: &Path,
    archive: &[u8],
    max_total_bytes: u64,
) -> Result<InstalledPackage, CompanionError> {
    let files = unzip(archive, max_total_bytes)?;
    let manifest_bytes = files
        .get("manifest.toml")
        .ok_or_else(|| CompanionError::package("missing manifest.toml"))?;
    let manifest: ManifestFile = toml::from_str(&String::from_utf8_lossy(manifest_bytes))
        .map_err(|err| CompanionError::package(err.to_string()))?;
    if manifest.package.format_version != FORMAT_VERSION {
        return Err(CompanionError::UnknownFormat {
            found: manifest.package.format_version,
            expected: FORMAT_VERSION,
        });
    }
    let kind = PackageKind::parse(&manifest.package.kind)?;
    let computed = content_digest(&files);
    let mut origin_unverified = true;
    if let Some(declared) = manifest
        .integrity
        .as_ref()
        .map(|i| i.digest.trim())
        .filter(|d| !d.is_empty())
    {
        if declared != computed {
            return Err(CompanionError::DigestMismatch);
        }
        origin_unverified = false;
    }
    let mut warnings = Vec::new();
    if let Some(persona) = files.get("soul/persona.md") {
        let text = String::from_utf8_lossy(persona);
        if text.contains("<script") || text.contains("javascript:") {
            warnings.push("persona contains script-like markup".to_owned());
        }
    }
    let dest = home.join(format!(
        "{}@{}",
        sanitize(&manifest.package.id),
        sanitize(&manifest.package.version)
    ));
    if dest.exists() {
        return Err(CompanionError::package(format!(
            "already installed at {}",
            dest.display()
        )));
    }
    write_tree(&dest, &files)?;
    store.record_package(
        &manifest.package.id,
        &manifest.package.version,
        kind.as_str(),
        &dest.to_string_lossy(),
        (!origin_unverified).then_some(computed.as_str()),
    )?;
    let _ = (
        manifest.contents,
        manifest.license,
        manifest.package.display_name,
    );
    Ok(InstalledPackage {
        id: manifest.package.id,
        version: manifest.package.version,
        kind,
        path: dest,
        digest: computed,
        origin_unverified,
        warnings,
    })
}

/// Create a soul row from an installed soul/character package.
pub fn soul_from_install(
    store: &CompanionStore,
    installed: &InstalledPackage,
) -> Result<crate::soul::Soul, CompanionError> {
    let soul_toml = fs::read_to_string(installed.path.join("soul/soul.toml")).unwrap_or_default();
    let parsed: SoulFile = if soul_toml.trim().is_empty() {
        SoulFile {
            identity: None,
            affect: None,
            voice: None,
            skills: None,
            proactive: None,
        }
    } else {
        toml::from_str(&soul_toml).map_err(|err| CompanionError::package(err.to_string()))?
    };
    let baseline = parsed.affect.and_then(|a| a.baseline).unwrap_or_default();
    let voice = parsed
        .voice
        .and_then(|v| (!v.voice_ref.is_empty()).then_some(v.voice_ref));
    let skills = parsed.skills.map(|s| s.refs).unwrap_or_default();
    let _tendency = parsed.proactive.map(|p| p.tendency);
    let _name = parsed.identity.map(|i| i.name);
    store.create_soul(&NewSoul {
        character_ref: format!("{}@{}", installed.id, installed.version),
        body_ref: None,
        voice_ref: voice,
        skill_refs: skills,
        affect_baseline: baseline,
    })
}

/// User-facing package name for `locale` (`en-US` / `ja`), falling back to the manifest default.
#[must_use]
pub fn localized_display_name(files: &BTreeMap<String, Vec<u8>>, locale: &str) -> String {
    let i18n_key = format!("i18n/{locale}.toml");
    if let Some(bytes) = files.get(&i18n_key)
        && let Ok(parsed) = toml::from_str::<LocaleFields>(&String::from_utf8_lossy(bytes))
        && !parsed.display_name.is_empty()
    {
        return parsed.display_name;
    }
    let Some(bytes) = files.get("manifest.toml") else {
        return String::new();
    };
    let Ok(manifest) = toml::from_str::<ManifestFile>(&String::from_utf8_lossy(bytes)) else {
        return String::new();
    };
    if let Some(localized) = manifest.locales.get(locale)
        && !localized.display_name.is_empty()
    {
        return localized.display_name.clone();
    }
    manifest.package.display_name
}

/// Resolve a localized display name from an installed package directory.
pub fn display_name_for_install(dir: &Path, locale: &str) -> Result<String, CompanionError> {
    let mut files = BTreeMap::new();
    collect_files(dir, dir, &mut files)?;
    let name = localized_display_name(&files, locale);
    if name.is_empty() {
        return Err(CompanionError::package("missing display name"));
    }
    Ok(name)
}

/// Bind an installed soul package to an installed body package (P-402 / P-8xx).
pub fn compose_soul_and_body(
    store: &CompanionStore,
    soul_pkg: &InstalledPackage,
    body_pkg: &InstalledPackage,
) -> Result<crate::soul::Soul, CompanionError> {
    if soul_pkg.kind != PackageKind::Soul && soul_pkg.kind != PackageKind::Character {
        return Err(CompanionError::package("soul side is not a soul package"));
    }
    if body_pkg.kind != PackageKind::Body && body_pkg.kind != PackageKind::Character {
        return Err(CompanionError::package("body side is not a body package"));
    }
    let mut soul = soul_from_install(store, soul_pkg)?;
    let body_id = ene_session::BodyId::new();
    store.set_body_ref(soul.id, Some(body_id))?;
    soul.body_ref = Some(body_id);
    Ok(soul)
}

/// Export an installed directory as a zip (no memories / logs).
pub fn export_dir(dir: &Path) -> Result<Vec<u8>, CompanionError> {
    let mut files = BTreeMap::new();
    collect_files(dir, dir, &mut files)?;
    pack_archive(&files)
}

/// Import Character Card V3 (JSON / PNG / CHARX) as a new `.enechar` install.
pub fn import_v3(
    store: &CompanionStore,
    home: &Path,
    bytes: &[u8],
    max_total_bytes: u64,
) -> Result<InstalledPackage, CompanionError> {
    let card = ene_card::load_card_from_bytes(bytes)
        .map_err(|err| CompanionError::package(err.to_string()))?;
    let name = if card.data.nickname.is_empty() {
        card.data.name.clone()
    } else {
        card.data.nickname.clone()
    };
    let id = format!("char.imported.{}", sanitize(&name));
    let mut persona = String::new();
    if !card.data.description.is_empty() {
        persona.push_str(&card.data.description);
        persona.push_str("\n\n");
    }
    if !card.data.personality.is_empty() {
        persona.push_str(&card.data.personality);
        persona.push_str("\n\n");
    }
    if !card.data.system_prompt.is_empty() {
        persona.push_str(&card.data.system_prompt);
    }
    let soul_toml = format!(
        "[identity]\nname = {}\nrole = \"companion\"\nlocale_default = \"en-US\"\n\n[persona]\nsource = \"persona.md\"\n",
        toml_quote(&name)
    );
    let body_toml = "[body]\nkind = \"text\"\n\n[expressions]\navailable = [\"happy\", \"calm\", \"sad\", \"angry\"]\n";
    let emotion_map = "[map.happy]\nexpression = \"happy\"\nintensity_scale = 1.0\n\n[map.calm]\nexpression = \"calm\"\nintensity_scale = 1.0\n";
    let mut files = BTreeMap::new();
    let mut manifest = format!(
        "[package]\nkind = \"character\"\nid = {id}\nversion = \"1.0.0\"\nformat_version = 1\ndisplay_name = {name}\n\n[contents]\nsoul = \"embedded\"\nbody = \"embedded\"\n\n[integrity]\ndigest = \"\"\n",
        id = toml_quote(&id),
        name = toml_quote(&name),
    );
    files.insert("soul/soul.toml".to_owned(), soul_toml.into_bytes());
    files.insert("soul/persona.md".to_owned(), persona.into_bytes());
    files.insert("body/body.toml".to_owned(), body_toml.bytes().collect());
    files.insert(
        "body/emotion_map.toml".to_owned(),
        emotion_map.bytes().collect(),
    );
    files.insert("manifest.toml".to_owned(), manifest.as_bytes().to_vec());
    let digest = content_digest(&files);
    manifest = manifest.replace("digest = \"\"", &format!("digest = \"{digest}\""));
    files.insert("manifest.toml".to_owned(), manifest.into_bytes());
    let archive = pack_archive(&files)?;
    install_archive(store, home, &archive, max_total_bytes)
}

fn unzip(bytes: &[u8], max_total: u64) -> Result<BTreeMap<String, Vec<u8>>, CompanionError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| CompanionError::package(err.to_string()))?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|err| CompanionError::package(err.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .enclosed_name()
            .ok_or_else(|| CompanionError::package("unsafe zip path"))?
            .to_string_lossy()
            .replace('\\', "/");
        let size = entry.size();
        if size > MAX_ENTRY_BYTES {
            return Err(CompanionError::PackageTooLarge(size));
        }
        total = total.saturating_add(size);
        if total > max_total {
            return Err(CompanionError::PackageTooLarge(total));
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|err| CompanionError::package(err.to_string()))?;
        files.insert(name, buf);
    }
    Ok(files)
}

fn write_tree(dest: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), CompanionError> {
    fs::create_dir_all(dest)?;
    for (name, bytes) in files {
        let path = dest.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    Ok(())
}

fn collect_files(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), CompanionError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(rel, fs::read(&path)?);
        }
    }
    Ok(())
}

fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

fn toml_quote(raw: &str) -> String {
    format!("\"{}\"", raw.replace('"', "\\\""))
}
