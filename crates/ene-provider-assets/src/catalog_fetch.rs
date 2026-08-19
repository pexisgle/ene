use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::catalog::AssetKind;
use crate::error::AssetError;
use crate::runtime_catalog::{
    CatalogArtifact, CatalogRelease, CatalogVariant, RuntimeCatalog, RuntimeCatalogAsset,
    RuntimePlatform,
};

const LLAMA_REPO: &str = "ggml-org/llama.cpp";
const VOICEVOX_REPO: &str = "VOICEVOX/voicevox_engine";

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

/// Fetch a runtime catalog for a host-managed plugin from GitHub Releases.
pub async fn fetch_runtime_catalog(plugin_id: &str) -> Result<RuntimeCatalog, AssetError> {
    let client = reqwest::Client::builder()
        .user_agent("ene-provider-assets")
        .build()
        .map_err(|err| AssetError::Download(err.to_string()))?;
    match plugin_id {
        "provider.gguf" => fetch_llama_catalog(&client).await,
        "provider.voicevox" => fetch_voicevox_catalog(&client).await,
        _ => Ok(RuntimeCatalog {
            plugin_id: plugin_id.to_owned(),
            assets: Vec::new(),
            fetched_at: now_stamp(),
            error: Some("no runtime catalog for plugin".to_owned()),
        }),
    }
}

async fn fetch_llama_catalog(client: &reqwest::Client) -> Result<RuntimeCatalog, AssetError> {
    let releases = fetch_releases(client, LLAMA_REPO, 30).await?;
    let platform = current_runtime_platform();
    let mut by_tag: BTreeMap<String, CatalogRelease> = BTreeMap::new();
    let mut cudart_by_backend: BTreeMap<String, CatalogArtifact> = BTreeMap::new();

    for release in &releases {
        for asset in &release.assets {
            if let Some(parsed) = parse_llama_cudart_asset(&asset.name) {
                if platform_matches(&parsed.platform, &platform) {
                    cudart_by_backend.insert(
                        parsed.variant_id.clone(),
                        gh_asset_to_artifact(asset, ArtifactExtract::ZipTree),
                    );
                }
                continue;
            }
            let Some(parsed) = parse_llama_main_asset(&asset.name) else {
                continue;
            };
            if !platform_matches(&parsed.platform, &platform) {
                continue;
            }
            if parsed.tag != release.tag_name {
                continue;
            }
            let entry = by_tag
                .entry(release.tag_name.clone())
                .or_insert_with(|| CatalogRelease {
                    tag: release.tag_name.clone(),
                    published_at: release.published_at.clone(),
                    variants: Vec::new(),
                });
            entry.variants.push(CatalogVariant {
                id: parsed.variant_id.clone(),
                label: parsed.label.clone(),
                platform: parsed.platform.clone(),
                backend: parsed.backend.clone(),
                recommended: parsed.recommended,
                artifacts: vec![gh_asset_to_artifact(asset, ArtifactExtract::ZipTree)],
                entry_binary: Some(parsed.member),
            });
        }
    }

    for release in by_tag.values_mut() {
        for variant in &mut release.variants {
            if let Some(companion) = cudart_by_backend.get(&variant.backend) {
                variant.artifacts.push(companion.clone());
            }
        }
    }

    let mut releases_vec: Vec<CatalogRelease> = by_tag.into_values().collect();
    releases_vec.sort_by(|a, b| b.tag.cmp(&a.tag));

    Ok(RuntimeCatalog {
        plugin_id: "provider.gguf".to_owned(),
        assets: vec![RuntimeCatalogAsset {
            id: "llama-server".to_owned(),
            kind: AssetKind::Sidecar,
            label: "llama-server".to_owned(),
            description: "Local GGUF inference engine (llama.cpp)".to_owned(),
            recommended: true,
            seams: Vec::new(),
            releases: releases_vec,
        }],
        fetched_at: now_stamp(),
        error: None,
    })
}

async fn fetch_voicevox_catalog(client: &reqwest::Client) -> Result<RuntimeCatalog, AssetError> {
    let releases = fetch_releases(client, VOICEVOX_REPO, 10).await?;
    let platform = current_runtime_platform();
    let entry_binary = if cfg!(windows) {
        "run.exe".to_owned()
    } else {
        "run".to_owned()
    };
    let mut by_tag: BTreeMap<String, CatalogRelease> = BTreeMap::new();

    for release in &releases {
        for asset in &release.assets {
            let Some(parsed) = parse_voicevox_vvpp(&asset.name) else {
                continue;
            };
            if !platform_matches(&parsed.platform, &platform) {
                continue;
            }
            if parsed.tag != release.tag_name {
                continue;
            }
            let row = by_tag
                .entry(release.tag_name.clone())
                .or_insert_with(|| CatalogRelease {
                    tag: release.tag_name.clone(),
                    published_at: release.published_at.clone(),
                    variants: Vec::new(),
                });
            row.variants.push(CatalogVariant {
                id: parsed.variant_id.clone(),
                label: parsed.label.clone(),
                platform: parsed.platform.clone(),
                backend: parsed.backend.clone(),
                recommended: parsed.recommended,
                artifacts: vec![gh_asset_to_artifact(asset, ArtifactExtract::ZipTree)],
                entry_binary: Some(entry_binary.clone()),
            });
        }
    }

    let mut releases_vec: Vec<CatalogRelease> = by_tag.into_values().collect();
    releases_vec.sort_by(|a, b| b.tag.cmp(&a.tag));

    Ok(RuntimeCatalog {
        plugin_id: "provider.voicevox".to_owned(),
        assets: vec![RuntimeCatalogAsset {
            id: "voicevox-engine".to_owned(),
            kind: AssetKind::Sidecar,
            label: "VOICEVOX Engine".to_owned(),
            description: "VOICEVOX-compatible TTS engine".to_owned(),
            recommended: true,
            seams: Vec::new(),
            releases: releases_vec,
        }],
        fetched_at: now_stamp(),
        error: None,
    })
}

async fn fetch_releases(
    client: &reqwest::Client,
    repo: &str,
    per_page: u32,
) -> Result<Vec<GhRelease>, AssetError> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={per_page}");
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| AssetError::Download(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AssetError::Download(format!(
            "GitHub API HTTP {}",
            response.status()
        )));
    }
    response
        .json::<Vec<GhRelease>>()
        .await
        .map_err(|err| AssetError::Download(err.to_string()))
}

enum ArtifactExtract {
    ZipTree,
}

impl ArtifactExtract {
    fn into_catalog(self) -> crate::runtime_catalog::ExtractMode {
        crate::runtime_catalog::ExtractMode::ZipTree
    }
}

fn gh_asset_to_artifact(asset: &GhAsset, extract: ArtifactExtract) -> CatalogArtifact {
    CatalogArtifact {
        url: asset.browser_download_url.clone(),
        sha256: asset
            .digest
            .as_deref()
            .and_then(parse_github_digest)
            .unwrap_or_default(),
        size_bytes: Some(asset.size),
        extract: extract.into_catalog(),
        dest: String::new(),
    }
}

fn parse_github_digest(digest: &str) -> Option<String> {
    digest.strip_prefix("sha256:").map(str::to_owned)
}

struct ParsedLlamaMain {
    tag: String,
    variant_id: String,
    label: String,
    backend: String,
    platform: RuntimePlatform,
    member: String,
    recommended: bool,
}

fn parse_llama_main_asset(name: &str) -> Option<ParsedLlamaMain> {
    let name = name.strip_suffix(".zip")?;
    let rest = name.strip_prefix("llama-")?;
    let (tag, rest) = rest.split_once("-bin-")?;
    if !tag.starts_with('b') {
        return None;
    }
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() < 2 {
        return None;
    }
    let arch = parts[parts.len() - 1];
    if arch != "x64" && arch != "arm64" {
        return None;
    }
    let os_token = parts[0];
    let (os, arch_norm) = match (os_token, arch) {
        ("win", "x64") => ("windows", "x86_64"),
        ("win", "arm64") => ("windows", "aarch64"),
        ("ubuntu", "x64") => ("linux", "x86_64"),
        ("ubuntu", "arm64") => ("linux", "aarch64"),
        _ => return None,
    };
    let backend_parts = &parts[1..parts.len() - 1];
    let (variant_id, label, backend, recommended) = if backend_parts.is_empty() {
        ("cpu".to_owned(), "CPU".to_owned(), "cpu".to_owned(), true)
    } else {
        let backend = backend_parts.join("-");
        let variant_id = backend.clone();
        let label = backend.to_uppercase();
        let recommended = backend == "avx2" || backend == "cpu";
        (variant_id, label, backend, recommended)
    };
    let member = if os == "windows" {
        "llama-server.exe".to_owned()
    } else {
        "llama-server".to_owned()
    };
    Some(ParsedLlamaMain {
        tag: tag.to_owned(),
        variant_id,
        label,
        backend,
        platform: RuntimePlatform {
            os: os.to_owned(),
            arch: arch_norm.to_owned(),
        },
        member,
        recommended,
    })
}

struct ParsedCudart {
    variant_id: String,
    platform: RuntimePlatform,
}

fn parse_llama_cudart_asset(name: &str) -> Option<ParsedCudart> {
    let name = name.strip_suffix(".zip")?;
    let rest = name.strip_prefix("cudart-llama-bin-")?;
    let (platform_token, rest) = rest.split_once('-')?;
    if platform_token != "win" {
        return None;
    }
    let rest = rest.strip_suffix("-x64")?;
    let variant_id = rest.to_owned();
    Some(ParsedCudart {
        variant_id,
        platform: RuntimePlatform {
            os: "windows".to_owned(),
            arch: "x86_64".to_owned(),
        },
    })
}

struct ParsedVoicevox {
    tag: String,
    platform: RuntimePlatform,
    variant_id: String,
    label: String,
    backend: String,
    recommended: bool,
}

fn parse_voicevox_vvpp(name: &str) -> Option<ParsedVoicevox> {
    let name = name.strip_suffix(".vvpp")?;
    let rest = name.strip_prefix("voicevox_engine-")?;
    if let Some(tag) = rest.strip_prefix("windows-cpu-") {
        return Some(voicevox_variant(
            tag,
            RuntimePlatform {
                os: "windows".to_owned(),
                arch: "x86_64".to_owned(),
            },
            "cpu",
            "CPU",
            "cpu",
            true,
        ));
    }
    if let Some(tag) = rest.strip_prefix("windows-directml-") {
        return Some(voicevox_variant(
            tag,
            RuntimePlatform {
                os: "windows".to_owned(),
                arch: "x86_64".to_owned(),
            },
            "directml",
            "GPU (DirectML)",
            "directml",
            false,
        ));
    }
    if let Some(tag) = rest.strip_prefix("linux-cpu-x64-") {
        return Some(voicevox_variant(
            tag,
            RuntimePlatform {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
            },
            "cpu",
            "CPU",
            "cpu",
            true,
        ));
    }
    if let Some(tag) = rest.strip_prefix("macos-x64-") {
        return Some(voicevox_variant(
            tag,
            RuntimePlatform {
                os: "macos".to_owned(),
                arch: "x86_64".to_owned(),
            },
            "cpu",
            "CPU (x64)",
            "cpu",
            true,
        ));
    }
    if let Some(tag) = rest.strip_prefix("macos-arm64-") {
        return Some(voicevox_variant(
            tag,
            RuntimePlatform {
                os: "macos".to_owned(),
                arch: "aarch64".to_owned(),
            },
            "cpu",
            "CPU (arm64)",
            "cpu",
            true,
        ));
    }
    None
}

fn voicevox_variant(
    tag: &str,
    platform: RuntimePlatform,
    variant_id: &str,
    label: &str,
    backend: &str,
    recommended: bool,
) -> ParsedVoicevox {
    ParsedVoicevox {
        tag: tag.to_owned(),
        platform,
        variant_id: variant_id.to_owned(),
        label: label.to_owned(),
        backend: backend.to_owned(),
        recommended,
    }
}

fn current_runtime_platform() -> RuntimePlatform {
    RuntimePlatform {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}

fn platform_matches(candidate: &RuntimePlatform, current: &RuntimePlatform) -> bool {
    candidate.os == current.os && candidate.arch == current.arch
}

fn now_stamp() -> Option<String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs().to_string())
}

pub const CACHE_TTL: Duration = Duration::from_hours(6);
/// Bump when catalog fetch / extract semantics change so disk cache is refetched.
pub const CATALOG_CACHE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCatalog {
    #[serde(default)]
    pub schema_version: u32,
    pub catalog: RuntimeCatalog,
    pub cached_at_secs: u64,
}

#[must_use]
pub fn catalog_cache_path(plugin_id: &str) -> std::path::PathBuf {
    ene_config::data_dir()
        .join("catalog-cache")
        .join(format!("{plugin_id}.json"))
}

pub fn load_cached_catalog(plugin_id: &str) -> Option<CachedCatalog> {
    let path = catalog_cache_path(plugin_id);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_cached_catalog(plugin_id: &str, catalog: &RuntimeCatalog) -> Result<(), AssetError> {
    let cached = wrap_cached_catalog(catalog.clone());
    let path = catalog_cache_path(plugin_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(&cached)?;
    crate::manifest::atomic_write(&path, raw.as_bytes()).map_err(AssetError::Io)
}

pub fn cache_stale(cached: &CachedCatalog) -> bool {
    if cached.schema_version != CATALOG_CACHE_SCHEMA_VERSION {
        return true;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    now.saturating_sub(cached.cached_at_secs) > CACHE_TTL.as_secs()
}

pub(crate) fn wrap_cached_catalog(catalog: RuntimeCatalog) -> CachedCatalog {
    CachedCatalog {
        schema_version: CATALOG_CACHE_SCHEMA_VERSION,
        catalog,
        cached_at_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_llama_win_avx2() {
        let parsed = parse_llama_main_asset("llama-b4282-bin-win-avx2-x64.zip").expect("parse");
        assert_eq!(parsed.tag, "b4282");
        assert_eq!(parsed.variant_id, "avx2");
        assert_eq!(parsed.member, "llama-server.exe");
    }

    #[test]
    fn parses_llama_ubuntu() {
        let parsed = parse_llama_main_asset("llama-b4282-bin-ubuntu-x64.zip").expect("parse");
        assert_eq!(parsed.platform.os, "linux");
        assert_eq!(parsed.variant_id, "cpu");
    }

    #[test]
    fn parses_voicevox_vvpp_name() {
        let parsed = parse_voicevox_vvpp("voicevox_engine-windows-cpu-0.25.2.vvpp").expect("parse");
        assert_eq!(parsed.tag, "0.25.2");
    }

    #[test]
    fn parses_voicevox_directml_vvpp() {
        let parsed =
            parse_voicevox_vvpp("voicevox_engine-windows-directml-0.25.2.vvpp").expect("parse");
        assert_eq!(parsed.tag, "0.25.2");
        assert_eq!(parsed.variant_id, "directml");
        assert_eq!(parsed.backend, "directml");
    }

    #[test]
    fn stale_cache_rejects_old_schema() {
        let cached = CachedCatalog {
            schema_version: 0,
            catalog: RuntimeCatalog {
                plugin_id: "provider.gguf".to_owned(),
                assets: Vec::new(),
                fetched_at: None,
                error: None,
            },
            cached_at_secs: u64::MAX,
        };
        assert!(cache_stale(&cached));
    }

    #[test]
    fn parses_cudart_companion() {
        let parsed =
            parse_llama_cudart_asset("cudart-llama-bin-win-cuda-12.4-x64.zip").expect("parse");
        assert_eq!(parsed.variant_id, "cuda-12.4");
    }
}
