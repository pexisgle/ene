//! `ene catalog` — key generation, signing, and verification of signed
//! artifact catalogs (the publisher side of the distribution pipeline).
//!
//! A catalog is a single self-contained JSON document ([`SignedCatalog`]):
//! the canonical payload, its Ed25519 signature, and the signing key id.
//! The host fetches it from `ArtifactConfig::catalog_url`, verifies it, and
//! installs the referenced artifacts through the content-addressable
//! installer. This module is the tooling that produces and checks those
//! documents; see `scripts/publish-catalog.sh` for the release flow.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::{SigningKey, VerifyingKey};
use ene_artifact::{
    ArtifactKind, ArtifactTarget, CatalogMetadata, CatalogVerifier, InstalledState, SignedCatalog,
    TrustedCatalogKeys, sign_catalog,
};
use rand_core::OsRng;
use serde::Deserialize;

use crate::cli::CatalogAction;
use crate::output::{ErrorCode, OutputError};

/// Parsed `--spec` file: catalog metadata plus one entry per artifact.
#[derive(Debug, Deserialize)]
struct CatalogSpec {
    /// Catalog version; `--version` on the CLI overrides this.
    #[serde(default = "default_version")]
    version: u64,
    /// Unix milliseconds. `0`/absent → `now + expires_hours`.
    #[serde(default)]
    expires_at_ms: u64,
    artifacts: Vec<SpecArtifact>,
}

/// One artifact entry in a spec file.
#[derive(Debug, Deserialize)]
struct SpecArtifact {
    id: String,
    /// `plugin`, `sidecar`, or `model`.
    kind: String,
    version: String,
    /// Ordered HTTPS mirror URLs; the first reachable one wins.
    urls: Vec<String>,
    /// Hex SHA-256 of the artifact bytes.
    sha256: String,
    /// Exact artifact size in bytes.
    size: u64,
}

fn default_version() -> u64 {
    1
}

/// Runs one `catalog` subcommand.
pub fn run(action: &CatalogAction, json: bool) -> Result<i32, OutputError> {
    let result = match action {
        CatalogAction::Keygen { out_dir } => keygen(out_dir),
        CatalogAction::Build {
            spec,
            key_id,
            key_hex,
            version,
            expires_hours,
            out,
        } => build(spec, key_id, key_hex, *version, *expires_hours, out),
        CatalogAction::Update {
            catalog,
            key_id,
            key_hex,
            version,
            expires_hours,
            out,
        } => update(
            catalog,
            key_id,
            key_hex,
            *version,
            *expires_hours,
            out.as_deref().unwrap_or(catalog),
        ),
        CatalogAction::Verify {
            catalog,
            key_id,
            key_hex,
            state,
        } => verify(catalog, key_id, key_hex, state.as_deref()),
    }?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("serialize: {e}")))?
        );
    } else {
        println!("{}", result.summary());
    }
    Ok(0)
}

/// Machine-readable result of a successful catalog operation.
#[derive(Debug, serde::Serialize)]
struct CatalogResult {
    op: &'static str,
    key_id: Option<String>,
    version: Option<u64>,
    expires_at_ms: Option<u64>,
    artifacts: Option<usize>,
    out: Option<String>,
    public_key_hex: Option<String>,
}

impl CatalogResult {
    fn summary(&self) -> String {
        match self.op {
            "keygen" => format!(
                "wrote keys to {} (key id: {})",
                self.out.as_deref().unwrap_or(""),
                self.key_id.as_deref().unwrap_or("")
            ),
            "verify" => "catalog OK (signature, expiry, and rollback checks passed)".to_string(),
            _ => {
                let mut parts = vec![format!(
                    "{}: catalog version {}",
                    self.op,
                    self.version.unwrap_or(0)
                )];
                if let Some(count) = self.artifacts {
                    parts.push(format!("{count} artifacts"));
                }
                if let Some(path) = &self.out {
                    parts.push(format!("wrote {path}"));
                }
                parts.join(", ")
            }
        }
    }
}

fn keygen(out_dir: &Path) -> Result<CatalogResult, OutputError> {
    let key = SigningKey::generate(&mut OsRng);
    fs::create_dir_all(out_dir).map_err(|e| {
        OutputError::new(
            ErrorCode::Runtime,
            format!("create key dir {}: {e}", out_dir.display()),
        )
    })?;
    let public_hex = hex::encode(key.verifying_key().to_bytes());
    let private_hex = hex::encode(key.to_bytes());
    let key_id = key_id_of(&key.verifying_key());
    fs::write(out_dir.join("key-id.txt"), format!("{key_id}\n"))
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("write key-id.txt: {e}")))?;
    fs::write(out_dir.join("private.hex"), format!("{private_hex}\n"))
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("write private.hex: {e}")))?;
    fs::write(out_dir.join("public.hex"), format!("{public_hex}\n"))
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("write public.hex: {e}")))?;
    Ok(CatalogResult {
        op: "keygen",
        key_id: Some(key_id),
        version: None,
        expires_at_ms: None,
        artifacts: None,
        out: Some(out_dir.display().to_string()),
        public_key_hex: Some(public_hex),
    })
}

fn build(
    spec_path: &Path,
    key_id: &str,
    key_hex: &str,
    version_override: Option<u64>,
    expires_hours: u64,
    out: &Path,
) -> Result<CatalogResult, OutputError> {
    let spec: CatalogSpec = read_json(spec_path)?;
    let key = parse_signing_key(key_hex)?;
    let mut artifacts = BTreeMap::new();
    for artifact in &spec.artifacts {
        validate_digest(&artifact.sha256)?;
        if artifact.urls.is_empty() {
            return Err(OutputError::new(
                ErrorCode::Usage,
                format!("artifact '{}' has no urls", artifact.id),
            ));
        }
        artifacts.insert(
            artifact.id.clone(),
            ArtifactTarget {
                version: artifact.version.clone(),
                kind: parse_kind(&artifact.kind)?,
                urls: artifact.urls.clone(),
                sha256: artifact.sha256.clone(),
                size: artifact.size,
            },
        );
    }
    let version = version_override.unwrap_or(spec.version);
    let expires_at_ms = if spec.expires_at_ms > 0 {
        spec.expires_at_ms
    } else {
        now_ms().saturating_add(expires_hours.saturating_mul(3600 * 1000))
    };
    let metadata = CatalogMetadata {
        version,
        expires_at_ms,
        artifacts,
    };
    let signed = sign_catalog(&metadata, key_id.to_string(), &key)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("sign catalog: {e}")))?;
    write_json(out, &signed)?;
    Ok(CatalogResult {
        op: "build",
        key_id: Some(key_id.to_string()),
        version: Some(version),
        expires_at_ms: Some(expires_at_ms),
        artifacts: Some(metadata.artifacts.len()),
        out: Some(out.display().to_string()),
        public_key_hex: None,
    })
}

fn update(
    catalog_path: &Path,
    key_id: &str,
    key_hex: &str,
    version_override: Option<u64>,
    expires_hours: Option<u64>,
    out: &Path,
) -> Result<CatalogResult, OutputError> {
    let signed: SignedCatalog = read_json(catalog_path)?;
    let mut metadata: CatalogMetadata = serde_json::from_slice(&signed.payload)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("parse catalog payload: {e}")))?;
    metadata.version = version_override.unwrap_or(metadata.version.saturating_add(1));
    if let Some(hours) = expires_hours {
        metadata.expires_at_ms = now_ms().saturating_add(hours.saturating_mul(3600 * 1000));
    }
    let key = parse_signing_key(key_hex)?;
    let resigned = sign_catalog(&metadata, key_id.to_string(), &key)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("re-sign catalog: {e}")))?;
    write_json(out, &resigned)?;
    Ok(CatalogResult {
        op: "update",
        key_id: Some(key_id.to_string()),
        version: Some(metadata.version),
        expires_at_ms: Some(metadata.expires_at_ms),
        artifacts: Some(metadata.artifacts.len()),
        out: Some(out.display().to_string()),
        public_key_hex: None,
    })
}

fn verify(
    catalog_path: &Path,
    key_id: &str,
    public_key_hex: &str,
    state: Option<&Path>,
) -> Result<CatalogResult, OutputError> {
    let signed: SignedCatalog = read_json(catalog_path)?;
    let key = VerifyingKey::from_bytes(
        &hex::decode(public_key_hex)
            .map_err(|e| {
                OutputError::new(ErrorCode::Usage, format!("invalid public key hex: {e}"))
            })?
            .try_into()
            .map_err(|_| OutputError::new(ErrorCode::Usage, "public key must be 32 bytes"))?,
    )
    .map_err(|e| OutputError::new(ErrorCode::Usage, format!("invalid public key: {e}")))?;
    let keys = TrustedCatalogKeys::from_hex(&[(key_id.to_string(), hex::encode(key.to_bytes()))])
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("key registry: {e}")))?;
    let verifier = CatalogVerifier::new(keys);
    let installed = state
        .map(load_installed_state)
        .transpose()?
        .unwrap_or_default()
        .artifacts
        .iter()
        .map(|(id, artifact)| {
            (
                id.clone(),
                (artifact.version.clone(), artifact.sha256.clone()),
            )
        })
        .collect();
    verifier
        .verify(&signed, &installed, now_ms())
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("verification failed: {e}")))?;
    Ok(CatalogResult {
        op: "verify",
        key_id: Some(key_id.to_string()),
        version: None,
        expires_at_ms: None,
        artifacts: None,
        out: None,
        public_key_hex: None,
    })
}

fn load_installed_state(path: &Path) -> Result<InstalledState, OutputError> {
    let bytes = fs::read(path).map_err(|e| {
        OutputError::new(
            ErrorCode::Runtime,
            format!("read installer state {}: {e}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        OutputError::new(
            ErrorCode::Runtime,
            format!("parse installer state {}: {e}", path.display()),
        )
    })
}

fn parse_signing_key(key_hex: &str) -> Result<SigningKey, OutputError> {
    let bytes = hex::decode(key_hex.trim())
        .map_err(|e| OutputError::new(ErrorCode::Usage, format!("invalid private key hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| OutputError::new(ErrorCode::Usage, "private key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&arr))
}

fn parse_kind(kind: &str) -> Result<ArtifactKind, OutputError> {
    match kind.trim() {
        "plugin" => Ok(ArtifactKind::Plugin),
        "sidecar" => Ok(ArtifactKind::Sidecar),
        "model" => Ok(ArtifactKind::Model),
        other => Err(OutputError::new(
            ErrorCode::Usage,
            format!("unknown artifact kind {other:?} (expected plugin|sidecar|model)"),
        )),
    }
}

fn validate_digest(digest: &str) -> Result<(), OutputError> {
    if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(OutputError::new(
            ErrorCode::Usage,
            format!("invalid SHA-256 digest {digest:?} (expected 64 hex chars)"),
        ))
    }
}

fn key_id_of(verifying: &VerifyingKey) -> String {
    let hex_key = hex::encode(verifying.to_bytes());
    hex_key[..16].to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, OutputError> {
    let bytes = fs::read(path).map_err(|e| {
        OutputError::new(ErrorCode::Runtime, format!("read {}: {e}", path.display()))
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("parse {}: {e}", path.display())))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), OutputError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| {
            OutputError::new(
                ErrorCode::Runtime,
                format!("create {}: {e}", parent.display()),
            )
        })?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("serialize: {e}")))?;
    fs::write(path, bytes)
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (String, String, String) {
        let key = SigningKey::generate(&mut OsRng);
        (
            key_id_of(&key.verifying_key()),
            hex::encode(key.to_bytes()),
            hex::encode(key.verifying_key().to_bytes()),
        )
    }

    fn spec_json() -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "artifacts": [{
                "id": "llama-server",
                "kind": "sidecar",
                "version": "1.0.0",
                "urls": ["https://example.test/llama-server"],
                "sha256": "ab".repeat(32),
                "size": 4
            }]
        })
    }

    #[test]
    fn keygen_build_verify_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (key_id, private_hex, public_hex) = keypair();
        let out_dir = dir.path().join("keys");
        run(
            &CatalogAction::Keygen {
                out_dir: out_dir.clone(),
            },
            false,
        )
        .expect("keygen");
        assert!(out_dir.join("key-id.txt").is_file());
        assert!(out_dir.join("private.hex").is_file());
        assert!(out_dir.join("public.hex").is_file());

        let spec = dir.path().join("spec.json");
        fs::write(&spec, serde_json::to_vec(&spec_json()).expect("spec json")).expect("write spec");
        let catalog = dir.path().join("catalog.json");
        run(
            &CatalogAction::Build {
                spec,
                key_id: key_id.clone(),
                key_hex: private_hex,
                version: None,
                expires_hours: 24,
                out: catalog.clone(),
            },
            false,
        )
        .expect("build");
        run(
            &CatalogAction::Verify {
                catalog,
                key_id,
                key_hex: public_hex,
                state: None,
            },
            false,
        )
        .expect("verify");
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (key_id, private_hex, _) = keypair();
        let (_, _, other_public) = keypair();
        let spec = dir.path().join("spec.json");
        fs::write(&spec, serde_json::to_vec(&spec_json()).expect("spec json")).expect("write spec");
        let catalog = dir.path().join("catalog.json");
        run(
            &CatalogAction::Build {
                spec,
                key_id: key_id.clone(),
                key_hex: private_hex,
                version: None,
                expires_hours: 24,
                out: catalog.clone(),
            },
            false,
        )
        .expect("build");
        let err = run(
            &CatalogAction::Verify {
                catalog,
                key_id,
                key_hex: other_public,
                state: None,
            },
            false,
        )
        .expect_err("wrong key must fail verification");
        assert!(err.to_string().contains("verification failed"));
    }

    #[test]
    fn build_rejects_bad_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_, private_hex, _) = keypair();
        let spec = dir.path().join("spec.json");
        let mut value = spec_json();
        value["artifacts"][0]["sha256"] = serde_json::Value::String("not-hex".to_string());
        fs::write(&spec, serde_json::to_vec(&value).expect("spec json")).expect("write spec");
        let err = run(
            &CatalogAction::Build {
                spec,
                key_id: "k".to_string(),
                key_hex: private_hex,
                version: None,
                expires_hours: 24,
                out: dir.path().join("catalog.json"),
            },
            false,
        )
        .expect_err("bad digest must fail");
        assert!(err.to_string().contains("SHA-256"));
    }
}
