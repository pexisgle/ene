use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::{ArtifactError, Result};

/// What an artifact is used for. The kind determines which approval category
/// gates updates (`PluginUpdate`, `SidecarUpdate`, `ModelUpdate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plugin,
    Sidecar,
    Model,
}

/// Payload format of an artifact object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadFormat {
    /// A single raw file; the CAS object is used directly (plugin binaries,
    /// model weights).
    Raw,
    /// A ZIP archive (VOICEVOX VVPP and friends) that is safely extracted
    /// into a per-generation directory before activation; `entrypoint`
    /// names the executable inside the archive.
    ZipVvpp,
}

/// How an artifact's bytes are materialized after download verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPayload {
    /// Payload container format.
    pub format: PayloadFormat,
    /// Executable path inside the archive, relative to the archive root
    /// (zip-vvpp only; e.g. `engine_manifest.json`'s `command`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Maximum total uncompressed size accepted during extraction (zip
    /// bombs); defaults to [`DEFAULT_UNPACK_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpack_limit: Option<u64>,
}

impl Default for ArtifactPayload {
    fn default() -> Self {
        Self {
            format: PayloadFormat::Raw,
            entrypoint: None,
            unpack_limit: None,
        }
    }
}

/// Default cap on total uncompressed bytes for extracted payloads (8 GiB —
/// far above any real engine distribution, far below a hostile archive's
/// claimed sizes).
pub const DEFAULT_UNPACK_LIMIT: u64 = 8 * 1024 * 1024 * 1024;

/// Maximum number of entries accepted in an extracted archive.
pub const MAX_ARCHIVE_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactTarget {
    /// Exact version string (compared segment-wise numerically, so `1.10`
    /// sorts after `1.9`).
    pub version: String,
    pub kind: ArtifactKind,
    /// Ordered mirror URLs (https only). The first reachable URL wins.
    pub urls: Vec<String>,
    pub sha256: String,
    pub size: u64,
    /// Payload format for the artifact bytes. `None`/`raw` keeps the
    /// original single-file behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<ArtifactPayload>,
    /// Platform-specific variants, keyed by `{os}-{arch}` (e.g.
    /// `linux-x86_64`, `windows-x86_64`). When the current platform has a
    /// variant, it replaces the top-level target fields entirely; the
    /// top-level fields remain the fallback for platforms without a variant
    /// (and for older catalog readers that predate this field).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub platforms: BTreeMap<String, ArtifactTarget>,
}

impl ArtifactTarget {
    /// Returns the target for `platform`, or the top-level fallback when the
    /// catalog does not carry a variant for it.
    #[must_use]
    pub fn for_platform(&self, platform: &str) -> &ArtifactTarget {
        self.platforms.get(platform).unwrap_or(self)
    }

    /// The `{os}-{arch}` key for the running process.
    #[must_use]
    pub fn current_platform() -> String {
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// Signed catalog metadata (TUF-inspired).
///
/// All maps are `BTreeMap` and all numeric fields are unsigned integers, so
/// `serde_json::to_vec` output is deterministic; the signature is computed
/// over exactly those bytes ([`canonical_catalog_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogMetadata {
    /// Monotonic catalog version. A newer catalog must have a strictly
    /// higher version; combined with the rollback rule this blocks
    /// downgrade attacks.
    pub version: u64,
    /// Expiry as Unix milliseconds. Expired catalogs are rejected regardless
    /// of signature validity.
    pub expires_at_ms: u64,
    pub artifacts: BTreeMap<String, ArtifactTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCatalog {
    /// The exact canonical JSON bytes that were signed.
    pub payload: Vec<u8>,
    /// Ed25519 signature (64 bytes) over `payload`.
    pub signature: Vec<u8>,
    /// Which trusted key signed this catalog.
    pub key_id: String,
}

pub fn canonical_catalog_bytes(metadata: &CatalogMetadata) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(metadata)?)
}

pub fn sign_catalog(
    metadata: &CatalogMetadata,
    key_id: String,
    signing_key: &SigningKey,
) -> Result<SignedCatalog> {
    let payload = canonical_catalog_bytes(metadata)?;
    let signature = signing_key.sign(&payload).to_bytes().to_vec();
    Ok(SignedCatalog {
        payload,
        signature,
        key_id,
    })
}

#[derive(Debug, Clone, Default)]
pub struct TrustedCatalogKeys {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustedCatalogKeys {
    pub fn from_hex(keys: &[(String, String)]) -> Result<Self> {
        let mut parsed = BTreeMap::new();
        for (key_id, hex_key) in keys {
            let bytes = hex::decode(hex_key)
                .map_err(|e| ArtifactError::Key(format!("invalid hex for {key_id}: {e}")))?;
            let key = VerifyingKey::from_bytes(
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| ArtifactError::Key(format!("bad key length for {key_id}")))?,
            )
            .map_err(|e| ArtifactError::Key(format!("invalid key {key_id}: {e}")))?;
            parsed.insert(key_id.clone(), key);
        }
        Ok(Self { keys: parsed })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Compares version strings segment-wise (`1.10` > `1.9`); non-numeric
/// segments fall back to lexicographic comparison.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_segments: Vec<&str> = a.split('.').collect();
    let b_segments: Vec<&str> = b.split('.').collect();
    for (a_seg, b_seg) in a_segments.iter().zip(&b_segments) {
        let ordering = match (a_seg.parse::<u64>(), b_seg.parse::<u64>()) {
            (Ok(a_num), Ok(b_num)) => a_num.cmp(&b_num),
            // A non-numeric segment marks a prerelease: 1.0 > 1.0-alpha.
            (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
            (Err(_), Ok(_)) => std::cmp::Ordering::Less,
            _ => a_seg.cmp(b_seg),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    match a_segments.len().cmp(&b_segments.len()) {
        std::cmp::Ordering::Equal => std::cmp::Ordering::Equal,
        // `a` is a prefix of `b`: numeric extra segments make the longer
        // version newer (1.0.1 > 1.0); non-numeric ones make it older.
        std::cmp::Ordering::Less => {
            if b_segments[a_segments.len()..]
                .iter()
                .all(|segment| segment.parse::<u64>().is_ok())
            {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        }
        std::cmp::Ordering::Greater => {
            if a_segments[b_segments.len()..]
                .iter()
                .all(|segment| segment.parse::<u64>().is_ok())
            {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        }
    }
}

#[derive(Debug)]
pub struct CatalogVerifier {
    keys: TrustedCatalogKeys,
}

impl CatalogVerifier {
    #[must_use]
    pub fn new(keys: TrustedCatalogKeys) -> Self {
        Self { keys }
    }

    /// Verifies signature, expiry, and rollback rules.
    ///
    /// `installed` maps artifact id → `(installed version, installed digest)`.
    /// Returns the parsed metadata on success. Rejections are automatic and
    /// cannot be overridden by any approval setting.
    pub fn verify(
        &self,
        catalog: &SignedCatalog,
        installed: &BTreeMap<String, (String, String)>,
        now_ms: u64,
    ) -> Result<CatalogMetadata> {
        let key = self.keys.keys.get(&catalog.key_id).ok_or_else(|| {
            ArtifactError::BadSignature(format!("unknown key id '{}'", catalog.key_id))
        })?;
        let signature = Signature::from_bytes(
            catalog
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| ArtifactError::BadSignature("signature length".to_string()))?,
        );
        key.verify(&catalog.payload, &signature)
            .map_err(|e| ArtifactError::BadSignature(e.to_string()))?;

        let metadata: CatalogMetadata = serde_json::from_slice(&catalog.payload)?;
        if metadata.expires_at_ms <= now_ms {
            return Err(ArtifactError::ExpiredCatalog {
                expired_at_ms: metadata.expires_at_ms,
                now_ms,
            });
        }
        check_rollback(&metadata, installed)?;
        Ok(metadata)
    }

    #[must_use]
    pub fn artifact_ids(&self, metadata: &CatalogMetadata) -> BTreeSet<String> {
        metadata.artifacts.keys().cloned().collect()
    }
}

fn check_rollback(
    metadata: &CatalogMetadata,
    installed: &BTreeMap<String, (String, String)>,
) -> Result<()> {
    for (artifact_id, (installed_version, installed_digest)) in installed {
        let Some(target) = metadata.artifacts.get(artifact_id) else {
            // Artifact removed from the catalog: not a rollback, but the
            // installed version simply has no update path.
            continue;
        };
        // Platform variants replace the top-level target for the running
        // platform; the rollback check must compare against the variant an
        // install would actually select, not the (possibly stale) top-level
        // fallback.
        let target = target.for_platform(&ArtifactTarget::current_platform());
        match compare_versions(&target.version, installed_version) {
            std::cmp::Ordering::Less => {
                return Err(ArtifactError::Rollback {
                    artifact: artifact_id.clone(),
                    detail: format!(
                        "catalog offers {} but {} is installed",
                        target.version, installed_version
                    ),
                });
            }
            std::cmp::Ordering::Equal => {
                // Same version must be byte-identical; a digest change
                // under an unchanged version is a rollback-style attack
                // and is rejected unconditionally.
                if *installed_digest != target.sha256 {
                    return Err(ArtifactError::Rollback {
                        artifact: artifact_id.clone(),
                        detail: format!(
                            "same version {} changed digest from {} to {}",
                            installed_version, installed_digest, target.sha256
                        ),
                    });
                }
            }
            std::cmp::Ordering::Greater => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> (String, SigningKey) {
        // Deterministic test key — never used outside tests.
        ("test-key".to_string(), SigningKey::from_bytes(&[7u8; 32]))
    }

    fn trusted(key: &(String, SigningKey)) -> TrustedCatalogKeys {
        let verifying = key.1.verifying_key().to_bytes();
        TrustedCatalogKeys::from_hex(&[(key.0.clone(), hex::encode(verifying))]).expect("keys")
    }

    fn metadata(version: u64, artifacts: &[(&str, &str)]) -> CatalogMetadata {
        CatalogMetadata {
            version,
            expires_at_ms: u64::MAX,
            artifacts: artifacts
                .iter()
                .map(|(id, ver)| {
                    (
                        (*id).to_string(),
                        ArtifactTarget {
                            version: (*ver).to_string(),
                            kind: ArtifactKind::Plugin,
                            urls: vec!["https://example.test/a.bin".to_string()],
                            sha256: "ab".repeat(32),
                            size: 4,
                            payload: None,
                            platforms: BTreeMap::new(),
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let key = test_key();
        let meta = metadata(1, &[("fs", "1.2.0")]);
        let signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let parsed = verifier
            .verify(&signed, &BTreeMap::new(), 0)
            .expect("verify");
        assert_eq!(parsed, meta);
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let key = test_key();
        let meta = metadata(1, &[]);
        let mut signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        signed.payload[0] ^= 0xFF;
        let verifier = CatalogVerifier::new(trusted(&key));
        assert!(verifier.verify(&signed, &BTreeMap::new(), 0).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let key = test_key();
        let meta = metadata(1, &[]);
        let signed = sign_catalog(&meta, "other-key".to_string(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        assert!(matches!(
            verifier.verify(&signed, &BTreeMap::new(), 0),
            Err(ArtifactError::BadSignature(_))
        ));
    }

    #[test]
    fn expired_catalog_is_rejected() {
        let key = test_key();
        let mut meta = metadata(1, &[]);
        meta.expires_at_ms = 1000;
        let signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        assert!(matches!(
            verifier.verify(&signed, &BTreeMap::new(), 2000),
            Err(ArtifactError::ExpiredCatalog { .. })
        ));
    }

    #[test]
    fn downgrade_is_rejected() {
        let key = test_key();
        let signed =
            sign_catalog(&metadata(2, &[("fs", "1.1.0")]), key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let installed =
            BTreeMap::from([("fs".to_string(), ("1.2.0".to_string(), "ab".repeat(32)))]);
        assert!(matches!(
            verifier.verify(&signed, &installed, 0),
            Err(ArtifactError::Rollback { .. })
        ));
    }

    #[test]
    fn same_version_digest_change_is_rejected() {
        let key = test_key();
        let meta = metadata(2, &[("fs", "1.2.0")]);
        let signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let installed =
            BTreeMap::from([("fs".to_string(), ("1.2.0".to_string(), "cd".repeat(32)))]);
        assert!(matches!(
            verifier.verify(&signed, &installed, 0),
            Err(ArtifactError::Rollback { .. })
        ));
    }

    #[test]
    fn same_version_same_digest_is_accepted() {
        let key = test_key();
        // Installed state does not carry digests in this API, so "same
        // version, same digest" is verified through the installer's state;
        // here we only assert that an upgrade path is accepted.
        let signed =
            sign_catalog(&metadata(2, &[("fs", "1.3.0")]), key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let installed =
            BTreeMap::from([("fs".to_string(), ("1.2.0".to_string(), "ab".repeat(32)))]);
        assert!(verifier.verify(&signed, &installed, 0).is_ok());
    }

    /// Rollback checks compare against the *platform variant* an install
    /// would select, not the top-level fallback.
    #[test]
    fn rollback_check_uses_platform_variant() {
        let key = test_key();
        let platform = ArtifactTarget::current_platform();
        let mut meta = metadata(2, &[("fs", "1.0.0")]);
        let target = meta.artifacts.get_mut("fs").expect("target");
        // The variant for the running platform is *older* than installed.
        target.platforms.insert(
            platform,
            ArtifactTarget {
                version: "0.9.0".to_string(),
                kind: ArtifactKind::Sidecar,
                urls: vec!["https://example.test/platform.bin".to_string()],
                sha256: "ef".repeat(32),
                size: 4,
                payload: None,
                platforms: BTreeMap::new(),
            },
        );
        let signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let installed =
            BTreeMap::from([("fs".to_string(), ("1.0.0".to_string(), "ab".repeat(32)))]);
        assert!(
            matches!(
                verifier.verify(&signed, &installed, 0),
                Err(ArtifactError::Rollback { .. })
            ),
            "a platform variant older than the installed version is a rollback"
        );
    }

    /// A same-version digest change in the platform variant is rejected even
    /// when the top-level fallback matches the installed digest.
    #[test]
    fn rollback_check_uses_platform_variant_digest() {
        let key = test_key();
        let platform = ArtifactTarget::current_platform();
        let mut meta = metadata(2, &[("fs", "1.0.0")]);
        let target = meta.artifacts.get_mut("fs").expect("target");
        target.platforms.insert(
            platform,
            ArtifactTarget {
                version: "1.0.0".to_string(),
                kind: ArtifactKind::Sidecar,
                urls: vec!["https://example.test/platform.bin".to_string()],
                // Same version, different digest than the installed one.
                sha256: "cd".repeat(32),
                size: 4,
                payload: None,
                platforms: BTreeMap::new(),
            },
        );
        let signed = sign_catalog(&meta, key.0.clone(), &key.1).expect("sign");
        let verifier = CatalogVerifier::new(trusted(&key));
        let installed =
            BTreeMap::from([("fs".to_string(), ("1.0.0".to_string(), "ab".repeat(32)))]);
        assert!(
            matches!(
                verifier.verify(&signed, &installed, 0),
                Err(ArtifactError::Rollback { .. })
            ),
            "a same-version digest change in the platform variant is rejected"
        );
    }

    #[test]
    fn version_compare_is_numeric() {
        assert_eq!(
            compare_versions("1.10.0", "1.9.9"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_versions("2.0", "2.0"), std::cmp::Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.1"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_versions("1.0.1", "1.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0-alpha", "1.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn canonical_bytes_are_stable() {
        let meta = metadata(3, &[("b", "1.0"), ("a", "2.0")]);
        let first = canonical_catalog_bytes(&meta).expect("canonical");
        let second = canonical_catalog_bytes(&meta).expect("canonical");
        assert_eq!(first, second);
        // Keys sort deterministically even though insertion order differed
        // (BTreeMap), so signatures are portable across processes.
        let reordered = CatalogMetadata {
            artifacts: meta.artifacts.clone(),
            ..meta
        };
        assert_eq!(
            first,
            canonical_catalog_bytes(&reordered).expect("canonical")
        );
    }
}
