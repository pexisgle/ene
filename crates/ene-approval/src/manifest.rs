use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::category::ApprovalCategory;
use crate::mode::ApprovalMode;

/// Slots are **logical** (`workspace`, `media`, `downloads`, …): the manifest
/// never records a real path. The host binds a slot to a canonical path only
/// through a user-approved grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsSlotDecl {
    pub name: String,
    pub purpose: String,
    pub read: bool,
    pub write: bool,
}

/// An exact HTTPS origin the plugin may talk to without dynamic-web approval.
///
/// The string is a serialized origin: `scheme://host[:port]` with no path,
/// query, or fragment. Only `https` origins are declarable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OriginDecl {
    pub origin: String,
}

/// Executable-artifact requirement: the plugin runs only with artifacts the
/// host resolves through the signed catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactRequirement {
    pub artifact_id: String,
    /// Version constraint (`=1.2.0`, `>=1.0`, `^1`). The host compares
    /// against the installed catalog target.
    pub version_constraint: String,
}

/// Sidecar requirement: an extra artifact the plugin expects the host to
/// install and keep running (e.g. a llama.cpp server).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SidecarRequirement {
    pub artifact_id: String,
    /// Version constraint, same syntax as [`ArtifactRequirement`].
    pub version_constraint: String,
}

/// Resource ceilings the plugin declares; the host enforces the ones its
/// sandbox supports and rejects requests that would exceed them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceLimits {
    /// Maximum memory in MiB (`0` = host default).
    pub max_memory_mb: u64,
    /// Maximum CPU share in percent (`0` = host default).
    pub max_cpu_percent: u64,
    /// Maximum child processes (`0` = host default).
    pub max_processes: u64,
    /// Maximum open file descriptors (`0` = host default).
    pub max_fds: u64,
    /// Maximum bytes in the per-plugin temp directory (`0` = host default).
    pub max_temp_bytes: u64,
    /// Maximum bytes per network-broker request (`0` = host default).
    pub max_network_bytes_per_request: u64,
}

/// The host uses these for audit labeling and for matching approvals to the
/// manifest; they are declarations, not grants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ManifestSideEffects {
    pub modifies_fs: bool,
    pub spawns_processes: bool,
    pub uses_network: bool,
    pub controls_browser: bool,
    pub reads_credentials: bool,
}

/// A capability that is not declared here can never be approved, even by an
/// `Allow` policy — the manifest layer is enforced before the approval layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestPermission {
    pub category: ApprovalCategory,
    /// Maximum mode the plugin may be granted (`Allow` here does **not**
    /// grant anything; it only makes automatic approval reachable).
    pub max: ApprovalMode,
}

/// Signed plugin manifest: the maximum capability surface of one plugin.
///
/// `payload` is the canonical JSON of the embedded [`PluginManifest`] (see
/// [`canonical_manifest_bytes`]); `signature` is an Ed25519 signature over
/// it. Built-in manifests shipped inside the host binary are trusted by
/// construction and carry no signature; third-party manifests must verify
/// against a trusted publisher key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedManifest {
    pub payload: Vec<u8>,
    /// Ed25519 signature (64 bytes), absent for built-in manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginManifest {
    pub schema_version: u32,
    /// Stable plugin id (matches `plugins.list.<name>` for built-ins).
    pub plugin_id: String,
    pub name: String,
    /// Publisher identifier; per-plugin approvals are bound to it.
    pub publisher: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_slots: Vec<FsSlotDecl>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_origins: Vec<OriginDecl>,
    #[serde(default)]
    pub dynamic_web: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<SidecarRequirement>,
    /// Host services (brokers) the plugin may open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_services: Vec<String>,
    #[serde(default)]
    pub side_effects: ManifestSideEffects,
    #[serde(default)]
    pub resource_limits: ResourceLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<ManifestPermission>,
}

/// All fields are declared struct fields (no maps), so output is
/// deterministic across processes and serde versions.
pub fn canonical_manifest_bytes(manifest: &PluginManifest) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(manifest)
}

/// The `manifest_digest` that FS grants and approvals are bound to.
pub fn manifest_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            plugin_id: "fs".to_string(),
            name: "Filesystem".to_string(),
            publisher: "ene".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            fs_slots: vec![FsSlotDecl {
                name: "workspace".to_string(),
                purpose: "User workspace files".to_string(),
                read: true,
                write: true,
            }],
            fixed_origins: vec![],
            dynamic_web: false,
            artifacts: vec![],
            sidecars: vec![],
            host_services: vec!["file".to_string()],
            side_effects: ManifestSideEffects {
                modifies_fs: true,
                ..ManifestSideEffects::default()
            },
            resource_limits: ResourceLimits::default(),
            permissions: vec![ManifestPermission {
                category: ApprovalCategory::FsRead,
                max: ApprovalMode::Allow,
            }],
        }
    }

    #[test]
    fn canonical_bytes_are_stable_and_digest_is_hex_sha256() {
        let manifest = sample_manifest();
        let first = canonical_manifest_bytes(&manifest).expect("canonical");
        let second = canonical_manifest_bytes(&manifest).expect("canonical");
        assert_eq!(first, second);
        let digest = manifest_sha256(&first);
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(digest, manifest_sha256(b"different"));
    }

    #[test]
    fn serde_round_trip_preserves_permissions() {
        let manifest = sample_manifest();
        let signed = SignedManifest {
            payload: canonical_manifest_bytes(&manifest).expect("canonical"),
            signature: Some(vec![1, 2, 3]),
            key_id: Some("key-1".to_string()),
        };
        let json = serde_json::to_value(&signed).expect("serialize");
        let back: SignedManifest = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, signed);
        assert_eq!(back.key_id.as_deref(), Some("key-1"));
    }

    #[test]
    fn builtin_manifest_round_trips_without_signature() {
        let manifest = sample_manifest();
        let signed = SignedManifest {
            payload: canonical_manifest_bytes(&manifest).expect("canonical"),
            signature: None,
            key_id: None,
        };
        let json = serde_json::to_value(&signed).expect("serialize");
        assert!(json.get("signature").is_none(), "None signature is skipped");
        let back: SignedManifest = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.signature, None);
    }
}
