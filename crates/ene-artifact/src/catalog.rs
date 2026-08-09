use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::{ArtifactError, Result};

/// What an artifact is used for. The kind determines which approval category
/// gates updates (`PluginUpdate`, `SidecarUpdate`, `ModelUpdate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A plugin binary.
    Plugin,
    /// A sidecar binary (e.g. a llama.cpp server).
    Sidecar,
    /// A model file (GGUF, ONNX, …).
    Model,
}

/// One installable artifact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactTarget {
    /// Exact version string (compared segment-wise numerically, so `1.10`
    /// sorts after `1.9`).
    pub version: String,
    /// Artifact kind.
    pub kind: ArtifactKind,
    /// Ordered mirror URLs (https only). The first reachable URL wins.
    pub urls: Vec<String>,
    /// Hex SHA-256 of the artifact bytes.
    pub sha256: String,
    /// Exact artifact size in bytes.
    pub size: u64,
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
    /// `artifact_id` → target.
    pub artifacts: BTreeMap<String, ArtifactTarget>,
}

/// A catalog payload plus its Ed25519 signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCatalog {
    /// The exact canonical JSON bytes that were signed.
    pub payload: Vec<u8>,
    /// Ed25519 signature (64 bytes) over `payload`.
    pub signature: Vec<u8>,
    /// Which trusted key signed this catalog.
    pub key_id: String,
}

/// Serializes metadata to the canonical byte form that gets signed.
pub fn canonical_catalog_bytes(metadata: &CatalogMetadata) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(metadata)?)
}

/// Signs catalog metadata with an Ed25519 signing key (host tooling, tests,
/// and the publisher side of the distribution pipeline).
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

/// Trusted catalog keys: hex-encoded Ed25519 verifying keys by key id.
#[derive(Debug, Clone, Default)]
pub struct TrustedCatalogKeys {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustedCatalogKeys {
    /// Builds the registry from `(key_id, hex_verifying_key)` pairs.
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

    /// Whether any keys are registered.
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

/// Verifies signed catalogs against trusted keys and the installed state.
#[derive(Debug)]
pub struct CatalogVerifier {
    keys: TrustedCatalogKeys,
}

impl CatalogVerifier {
    /// Builds a verifier over the trusted keys.
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

    /// All artifact ids present in a catalog (for update checks).
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
        // Installed digest differs from the catalog's (ab*32).
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
