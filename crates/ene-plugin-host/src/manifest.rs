//! Plugin manifest verification and the built-in manifest table.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use ene_approval::{
    ApprovalCategory, ApprovalMode, FsSlotDecl, ManifestPermission, OriginDecl, PluginManifest,
    ResourceLimits, SignedManifest,
};

use crate::config::TrustedPublisherConfig;

/// Manifest verification errors. All are fatal for the plugin's approval
/// surface: an unverifiable manifest grants nothing.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The manifest carries a signature but no key id.
    #[error("manifest for '{0}' has a signature but no key id")]
    MissingKeyId(String),
    /// The manifest's publisher key is not trusted.
    #[error("manifest publisher '{publisher}' (key '{key_id}') is not trusted")]
    UntrustedPublisher {
        /// Publisher id from the manifest.
        publisher: String,
        /// Key id referenced by the manifest.
        key_id: String,
    },
    /// The signature did not verify.
    #[error("manifest signature verification failed for '{0}': {1}")]
    BadSignature(String, String),
    /// A built-in manifest is missing its payload.
    #[error("built-in manifest '{0}' has no payload")]
    MissingPayload(String),
    /// The manifest payload is not valid JSON.
    #[error("manifest payload for '{0}' is not valid JSON: {1}")]
    InvalidPayload(String, String),
    /// The manifest's plugin id does not match the entry name.
    #[error("manifest plugin_id '{found}' does not match entry name '{expected}'")]
    IdMismatch {
        /// The plugin id from the manifest.
        found: String,
        /// The entry name the manifest is attached to.
        expected: String,
    },
    /// A signed manifest's publisher does not match the trusted key it used.
    #[error("manifest publisher '{publisher}' does not match signing key '{key_id}'")]
    PublisherMismatch {
        /// Publisher id from the manifest.
        publisher: String,
        /// Trusted key id used to verify the signature.
        key_id: String,
    },
    /// An unsigned manifest for a built-in name differs from the host copy.
    #[error("unsigned manifest for built-in '{0}' does not match the embedded manifest")]
    BuiltinMismatch(String),
    /// A logical FS slot in the manifest was requested but never granted.
    #[error("plugin has no {access} grant for slot '{slot}'")]
    MissingGrant {
        /// Plugin name.
        plugin: String,
        /// Slot name.
        slot: String,
        /// Required access.
        access: &'static str,
    },
}

/// Verified manifests plus the trusted-publisher key registry.
#[derive(Debug, Default)]
pub struct ManifestStore {
    keys: BTreeMap<String, VerifyingKey>,
}

impl ManifestStore {
    /// Builds the key registry from configuration.
    #[must_use]
    pub fn new(publishers: &[TrustedPublisherConfig]) -> Self {
        let mut keys = BTreeMap::new();
        for publisher in publishers {
            match hex::decode(&publisher.public_key_hex)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                .and_then(|key_bytes| VerifyingKey::from_bytes(&key_bytes).ok())
            {
                Some(key) => {
                    keys.insert(publisher.publisher.clone(), key);
                }
                None => {
                    tracing::warn!(
                        publisher = %publisher.publisher,
                        "ignoring invalid trusted publisher key"
                    );
                }
            }
        }
        Self { keys }
    }

    /// Verifies `signed` and parses the manifest.
    ///
    /// Signed manifests must verify against a trusted publisher key.
    /// Unsigned manifests are accepted only for known built-in plugins (the
    /// host ships their manifests in its own binary).
    pub fn verify(
        &self,
        signed: &SignedManifest,
        entry_name: &str,
    ) -> Result<PluginManifest, ManifestError> {
        let manifest: PluginManifest = if let Some(signature) = &signed.signature {
            let key_id = signed
                .key_id
                .as_deref()
                .ok_or_else(|| ManifestError::MissingKeyId(entry_name.to_string()))?;
            let key = self
                .keys
                .get(key_id)
                .ok_or_else(|| ManifestError::UntrustedPublisher {
                    publisher: key_id.to_string(),
                    key_id: key_id.to_string(),
                })?;
            let signature =
                Signature::from_bytes(signature.as_slice().try_into().map_err(|_| {
                    ManifestError::BadSignature(
                        entry_name.to_string(),
                        "signature length".to_string(),
                    )
                })?);
            key.verify(&signed.payload, &signature)
                .map_err(|e| ManifestError::BadSignature(entry_name.to_string(), e.to_string()))?;
            let manifest: PluginManifest =
                serde_json::from_slice(&signed.payload).map_err(|e| {
                    ManifestError::InvalidPayload(entry_name.to_string(), e.to_string())
                })?;
            if manifest.publisher != key_id {
                return Err(ManifestError::PublisherMismatch {
                    publisher: manifest.publisher,
                    key_id: key_id.to_string(),
                });
            }
            manifest
        } else {
            let Some(expected) = builtin_manifest(entry_name) else {
                return Err(ManifestError::MissingPayload(entry_name.to_string()));
            };
            let manifest: PluginManifest =
                serde_json::from_slice(&signed.payload).map_err(|e| {
                    ManifestError::InvalidPayload(entry_name.to_string(), e.to_string())
                })?;
            if manifest != expected {
                return Err(ManifestError::BuiltinMismatch(entry_name.to_string()));
            }
            manifest
        };
        if manifest.plugin_id != entry_name {
            return Err(ManifestError::IdMismatch {
                found: manifest.plugin_id.clone(),
                expected: entry_name.to_string(),
            });
        }
        Ok(manifest)
    }

    /// The manifest digest approvals and grants are bound to.
    #[must_use]
    pub fn digest(signed: &SignedManifest) -> String {
        ene_approval::manifest_sha256(&signed.payload)
    }
}

/// One approved filesystem grant for a plugin generation.
#[derive(Debug, Clone)]
pub struct FsGrant {
    /// Logical slot name.
    pub slot: String,
    /// Canonical real path.
    pub path: PathBuf,
    /// Read access granted.
    pub read: bool,
    /// Write access granted.
    pub write: bool,
}

/// Resolves a logical path (`slot/rest/...`) against a plugin's grants.
///
/// `rest` must stay inside the granted directory: absolute paths, `..`
/// traversal, and empty segments are rejected. The returned path is not yet
/// canonicalized — callers canonicalize and re-check containment to defeat
/// symlink swaps.
pub fn resolve_grant_path<'a>(
    grants: &'a [FsGrant],
    logical_path: &str,
    need_write: bool,
) -> Result<(&'a FsGrant, PathBuf), ManifestError> {
    let mut segments = logical_path.split('/').filter(|s| !s.is_empty());
    let slot = segments.next().ok_or_else(|| ManifestError::MissingGrant {
        plugin: "?".to_string(),
        slot: logical_path.to_string(),
        access: "read",
    })?;
    if segments.any(|segment| segment == ".." || segment.contains('\\')) {
        return Err(ManifestError::MissingGrant {
            plugin: "?".to_string(),
            slot: logical_path.to_string(),
            access: if need_write { "write" } else { "read" },
        });
    }
    let grant = grants
        .iter()
        .find(|grant| grant.slot == slot)
        .ok_or_else(|| ManifestError::MissingGrant {
            plugin: "?".to_string(),
            slot: slot.to_string(),
            access: if need_write { "write" } else { "read" },
        })?;
    let access_ok = if need_write { grant.write } else { grant.read };
    if !access_ok {
        return Err(ManifestError::MissingGrant {
            plugin: "?".to_string(),
            slot: slot.to_string(),
            access: if need_write { "write" } else { "read" },
        });
    }
    let rest = logical_path
        .strip_prefix(slot)
        .unwrap_or_default()
        .trim_start_matches('/');
    Ok((grant, grant.path.join(rest)))
}

/// Canonicalizes `candidate` and verifies it stays within `root` (symlink
/// escapes are rejected). The root itself is canonicalized once.
pub fn canonical_within(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let canonical = candidate.canonicalize().ok()?;
    canonical.starts_with(&root).then_some(canonical)
}

/// Canonicalizes `path`, falling back to the nearest existing ancestor when
/// the final components do not exist yet (write targets). Symlinks in the
/// existing prefix are resolved; trailing components are appended verbatim.
#[must_use]
pub fn canonicalize_or_nearest(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut result = canonical;
            for component in tail.iter().rev() {
                result.push(component);
            }
            return result;
        }
        let Some(name) = current.file_name().map(std::ffi::OsStr::to_os_string) else {
            return path.to_path_buf();
        };
        tail.push(name);
        if !current.pop() {
            return path.to_path_buf();
        }
    }
}

/// Resolves an absolute path against a plugin's grants.
///
/// The canonicalized target (or its nearest existing ancestor for
/// not-yet-created write targets) must lie inside a grant whose access
/// includes the requested operation. Symlink escapes are rejected by the
/// canonicalization itself.
pub fn resolve_grant_abs(
    grants: &[FsGrant],
    path: &Path,
    need_write: bool,
) -> Result<PathBuf, ManifestError> {
    let canonical = canonicalize_or_nearest(path);
    let access = if need_write { "write" } else { "read" };
    let grant = grants.iter().find(|grant| {
        canonical.starts_with(&grant.path) && if need_write { grant.write } else { grant.read }
    });
    let Some(_grant) = grant else {
        return Err(ManifestError::MissingGrant {
            plugin: "?".to_string(),
            slot: path.to_string_lossy().into_owned(),
            access,
        });
    };
    Ok(canonical)
}

/// The manifest for a built-in plugin, shipped inside the host binary.
///
/// These are trusted by construction (no signature needed). Third-party
/// plugins must supply a signed manifest in `plugins.list.<name>.manifest`.
#[must_use]
pub fn builtin_manifest(name: &str) -> Option<PluginManifest> {
    let manifest = match name {
        "app" => PluginManifest {
            plugin_id: "app".into(),
            name: "App control".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Window, input, and screenshot control".into()),
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: false,
                controls_browser: true,
                reads_credentials: false,
            },
            permissions: permissions(&[(ApprovalCategory::Browser, ApprovalMode::Allow)]),
            host_services: vec!["platform".into()],
            ..base("app")
        },
        "browser" => PluginManifest {
            plugin_id: "browser".into(),
            name: "Browser automation".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Chrome automation".into()),
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: true,
                uses_network: false,
                controls_browser: true,
                reads_credentials: false,
            },
            permissions: permissions(&[(ApprovalCategory::Browser, ApprovalMode::Allow)]),
            host_services: vec!["process".into(), "platform".into()],
            ..base("browser")
        },
        "fs" => PluginManifest {
            plugin_id: "fs".into(),
            name: "Filesystem".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("File read/write/edit, search, shell, undo".into()),
            fs_slots: vec![FsSlotDecl {
                name: "workspace".into(),
                purpose: "User workspace files".into(),
                read: true,
                write: true,
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: true,
                spawns_processes: true,
                uses_network: false,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::FsRead, ApprovalMode::Allow),
                (ApprovalCategory::FsCreate, ApprovalMode::Allow),
                (ApprovalCategory::FsModify, ApprovalMode::Allow),
                (ApprovalCategory::FsDelete, ApprovalMode::Allow),
                (ApprovalCategory::Shell, ApprovalMode::Allow),
                (ApprovalCategory::ProcessSpawn, ApprovalMode::Allow),
            ]),
            host_services: vec!["file".into(), "process".into()],
            ..base("fs")
        },
        "web" => PluginManifest {
            plugin_id: "web".into(),
            name: "Web".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Fetch URLs and search the web".into()),
            dynamic_web: true,
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::DynamicHttps, ApprovalMode::Allow),
                (ApprovalCategory::WebFileSave, ApprovalMode::Allow),
                (ApprovalCategory::CredentialUse, ApprovalMode::Allow),
            ]),
            host_services: vec!["network".into(), "file".into()],
            ..base("web")
        },
        "utility" => PluginManifest {
            plugin_id: "utility".into(),
            name: "Utility".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Notifications, todos, timers, system info".into()),
            permissions: permissions(&[(ApprovalCategory::Platform, ApprovalMode::Allow)]),
            host_services: vec!["platform".into()],
            ..base("utility")
        },
        "openai" | "openai-tts" => PluginManifest {
            plugin_id: name.into(),
            name: "OpenAI".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("OpenAI chat, embeddings, and TTS".into()),
            dynamic_web: true,
            fixed_origins: vec![OriginDecl {
                origin: "https://api.openai.com".into(),
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::FixedOriginNetwork, ApprovalMode::Allow),
                (ApprovalCategory::DynamicHttps, ApprovalMode::Allow),
                (ApprovalCategory::CredentialUse, ApprovalMode::Allow),
            ]),
            host_services: vec!["network".into()],
            ..base(name)
        },
        "anthropic" => PluginManifest {
            plugin_id: "anthropic".into(),
            name: "Anthropic".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Anthropic Messages API".into()),
            fixed_origins: vec![OriginDecl {
                origin: "https://api.anthropic.com".into(),
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::FixedOriginNetwork, ApprovalMode::Allow),
                (ApprovalCategory::CredentialUse, ApprovalMode::Allow),
            ]),
            host_services: vec!["network".into()],
            ..base("anthropic")
        },
        "elevenlabs" => PluginManifest {
            plugin_id: "elevenlabs".into(),
            name: "ElevenLabs".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("ElevenLabs TTS".into()),
            dynamic_web: true,
            fixed_origins: vec![OriginDecl {
                origin: "https://api.elevenlabs.io".into(),
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::FixedOriginNetwork, ApprovalMode::Allow),
                (ApprovalCategory::DynamicHttps, ApprovalMode::Allow),
            ]),
            host_services: vec!["network".into()],
            ..base("elevenlabs")
        },
        "edge-tts" => PluginManifest {
            plugin_id: "edge-tts".into(),
            name: "Edge TTS".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Microsoft Edge Read Aloud".into()),
            fixed_origins: vec![OriginDecl {
                origin: "https://speech.platform.bing.com".into(),
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[(
                ApprovalCategory::FixedOriginNetwork,
                ApprovalMode::Allow,
            )]),
            host_services: vec!["network".into()],
            ..base("edge-tts")
        },
        "geo" => PluginManifest {
            plugin_id: "geo".into(),
            name: "Geo".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Location, weather, timezone".into()),
            dynamic_web: true,
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[(ApprovalCategory::DynamicHttps, ApprovalMode::Allow)]),
            host_services: vec!["network".into()],
            ..base("geo")
        },
        "homeassistant" => PluginManifest {
            plugin_id: "homeassistant".into(),
            name: "Home Assistant".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Home Assistant state and control".into()),
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: false,
                spawns_processes: false,
                uses_network: true,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[]),
            // LAN access is denied in v1 (see the sandbox plan); this plugin
            // cannot reach its engine until a LAN approval path exists.
            host_services: vec![],
            ..base("homeassistant")
        },
        "git" => PluginManifest {
            plugin_id: "git".into(),
            name: "Git".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Git repository operations".into()),
            fs_slots: vec![FsSlotDecl {
                name: "workspace".into(),
                purpose: "Repository files".into(),
                read: true,
                write: false,
            }],
            side_effects: ene_approval::ManifestSideEffects {
                modifies_fs: true,
                spawns_processes: true,
                uses_network: false,
                controls_browser: false,
                reads_credentials: false,
            },
            permissions: permissions(&[
                (ApprovalCategory::FsRead, ApprovalMode::Allow),
                (ApprovalCategory::ProcessSpawn, ApprovalMode::Allow),
            ]),
            host_services: vec!["file".into(), "process".into()],
            ..base("git")
        },
        "calc" | "counter" | "random" => PluginManifest {
            plugin_id: name.into(),
            name: name.into(),
            publisher: "ene".into(),
            version: "1".into(),
            permissions: permissions(&[]),
            host_services: vec![],
            ..base(name)
        },
        "calendar" => PluginManifest {
            plugin_id: "calendar".into(),
            name: "Calendar".into(),
            publisher: "ene".into(),
            version: "1".into(),
            description: Some("Calendar accounts and events".into()),
            permissions: permissions(&[]),
            host_services: vec![],
            ..base("calendar")
        },
        // Local providers: no direct network, no user files, no artifacts
        // until the signed catalog ships them.
        "llama-cpp" | "llama-server" | "local-llm" | "onnx" | "whisper" | "kokoro" | "voicevox" => {
            PluginManifest {
                plugin_id: name.into(),
                name: name.into(),
                publisher: "ene".into(),
                version: "1".into(),
                description: Some("Local inference provider".into()),
                permissions: permissions(&[]),
                host_services: vec![],
                ..base(name)
            }
        }
        _ => return None,
    };
    Some(manifest)
}

fn base(plugin_id: &str) -> PluginManifest {
    PluginManifest {
        schema_version: 1,
        plugin_id: plugin_id.into(),
        name: plugin_id.into(),
        publisher: "ene".into(),
        version: "1".into(),
        description: None,
        fs_slots: Vec::new(),
        fixed_origins: Vec::new(),
        dynamic_web: false,
        artifacts: Vec::new(),
        sidecars: Vec::new(),
        host_services: Vec::new(),
        side_effects: ene_approval::ManifestSideEffects::default(),
        resource_limits: ResourceLimits::default(),
        permissions: Vec::new(),
    }
}

fn permissions(entries: &[(ApprovalCategory, ApprovalMode)]) -> Vec<ManifestPermission> {
    entries
        .iter()
        .map(|&(category, max)| ManifestPermission { category, max })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_manifests_cover_default_plugins() {
        for name in [
            "app",
            "browser",
            "calc",
            "calendar",
            "counter",
            "fs",
            "geo",
            "git",
            "homeassistant",
            "random",
            "utility",
            "web",
            "anthropic",
            "edge-tts",
            "elevenlabs",
            "kokoro",
            "llama-cpp",
            "llama-server",
            "local-llm",
            "onnx",
            "openai",
            "openai-tts",
            "voicevox",
            "whisper",
        ] {
            assert!(builtin_manifest(name).is_some(), "missing builtin: {name}");
        }
        assert!(builtin_manifest("not-a-plugin").is_none());
    }

    #[test]
    fn builtin_manifests_are_stable_and_digestable() {
        let manifest = builtin_manifest("fs").expect("fs manifest");
        let bytes = ene_approval::canonical_manifest_bytes(&manifest).expect("canonical");
        assert_eq!(ene_approval::manifest_sha256(&bytes).len(), 64);
    }

    #[test]
    fn resolve_grant_path_rejects_traversal() {
        let grants = vec![FsGrant {
            slot: "workspace".into(),
            path: PathBuf::from("/tmp/root"),
            read: true,
            write: true,
        }];
        assert!(resolve_grant_path(&grants, "workspace/notes.txt", false).is_ok());
        assert!(resolve_grant_path(&grants, "workspace/sub/dir/f.txt", false).is_ok());
        assert!(resolve_grant_path(&grants, "workspace/../etc/passwd", false).is_err());
        assert!(resolve_grant_path(&grants, "other/file", false).is_err());
        assert!(resolve_grant_path(&grants, "/etc/passwd", false).is_err());
        assert!(resolve_grant_path(&grants, "workspace", true).is_ok());
    }

    #[test]
    fn verify_rejects_unsigned_third_party_manifest() {
        let store = ManifestStore::default();
        let manifest = builtin_manifest("fs").expect("fs");
        let signed = SignedManifest {
            payload: ene_approval::canonical_manifest_bytes(&manifest).expect("canonical"),
            signature: None,
            key_id: None,
        };
        assert!(matches!(
            store.verify(&signed, "third-party"),
            Err(ManifestError::MissingPayload(_))
        ));
        // A builtin name is fine unsigned.
        assert!(store.verify(&signed, "fs").is_ok());
    }

    #[test]
    fn verify_rejects_unsigned_manifest_that_reuses_builtin_name() {
        let store = ManifestStore::default();
        let mut manifest = builtin_manifest("fs").expect("fs");
        manifest.permissions.clear();
        let signed = SignedManifest {
            payload: ene_approval::canonical_manifest_bytes(&manifest).expect("canonical"),
            signature: None,
            key_id: None,
        };
        assert!(matches!(
            store.verify(&signed, "fs"),
            Err(ManifestError::BuiltinMismatch(name)) if name == "fs"
        ));
    }

    #[test]
    fn verify_rejects_untrusted_signature() {
        let store = ManifestStore::default();
        let manifest = builtin_manifest("fs").expect("fs");
        let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        use ed25519_dalek::Signer;
        let payload = ene_approval::canonical_manifest_bytes(&manifest).expect("canonical");
        let signed = SignedManifest {
            signature: Some(key.sign(&payload).to_bytes().to_vec()),
            key_id: Some("unknown-publisher".into()),
            payload,
        };
        assert!(matches!(
            store.verify(&signed, "fs"),
            Err(ManifestError::UntrustedPublisher { .. })
        ));
    }

    #[test]
    fn verify_accepts_trusted_signature() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[11u8; 32]);
        let verifying = key.verifying_key().to_bytes();
        let store = ManifestStore::new(&[TrustedPublisherConfig {
            publisher: "acme".into(),
            public_key_hex: hex::encode(verifying),
        }]);
        let mut manifest = builtin_manifest("fs").expect("fs");
        manifest.publisher = "acme".into();
        manifest.plugin_id = "myfs".into();
        use ed25519_dalek::Signer;
        let payload = ene_approval::canonical_manifest_bytes(&manifest).expect("canonical");
        let signed = SignedManifest {
            signature: Some(key.sign(&payload).to_bytes().to_vec()),
            key_id: Some("acme".into()),
            payload,
        };
        let verified = store.verify(&signed, "myfs").expect("verify");
        assert_eq!(verified.publisher, "acme");
    }

    #[test]
    fn verify_rejects_signed_manifest_with_mismatched_publisher() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[12u8; 32]);
        let verifying = key.verifying_key().to_bytes();
        let store = ManifestStore::new(&[TrustedPublisherConfig {
            publisher: "acme".into(),
            public_key_hex: hex::encode(verifying),
        }]);
        let mut manifest = builtin_manifest("fs").expect("fs");
        manifest.publisher = "other".into();
        manifest.plugin_id = "myfs".into();
        use ed25519_dalek::Signer;
        let payload = ene_approval::canonical_manifest_bytes(&manifest).expect("canonical");
        let signed = SignedManifest {
            signature: Some(key.sign(&payload).to_bytes().to_vec()),
            key_id: Some("acme".into()),
            payload,
        };
        assert!(matches!(
            store.verify(&signed, "myfs"),
            Err(ManifestError::PublisherMismatch { publisher, key_id })
                if publisher == "other" && key_id == "acme"
        ));
    }

    #[test]
    fn resolve_grant_abs_matches_by_canonical_containment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("granted");
        std::fs::create_dir_all(root.join("sub")).expect("mkdir");
        std::fs::write(root.join("sub").join("notes.txt"), b"x").expect("write");

        let grants = vec![FsGrant {
            slot: "workspace".into(),
            path: root.canonicalize().expect("canonical"),
            read: true,
            write: true,
        }];
        let inside = resolve_grant_abs(&grants, &root.join("sub").join("notes.txt"), false)
            .expect("inside grant");
        assert!(inside.ends_with("notes.txt"));
        // A not-yet-created write target resolves through its existing
        // ancestor (create flow).
        let new_target = resolve_grant_abs(
            &grants,
            &root.join("sub").join("new").join("file.txt"),
            true,
        )
        .expect("write target inside grant");
        assert!(new_target.ends_with("file.txt"));
        // Outside the grant: rejected for both read and write.
        let outside = dir.path().join("other").join("secret.txt");
        std::fs::create_dir_all(outside.parent().expect("parent")).expect("mkdir");
        std::fs::write(&outside, b"secret").expect("write");
        assert!(resolve_grant_abs(&grants, &outside, false).is_err());
        assert!(resolve_grant_abs(&grants, &outside, true).is_err());
        // Read-only grant rejects writes.
        let read_only = vec![FsGrant {
            slot: "workspace".into(),
            path: root.canonicalize().expect("canonical"),
            read: true,
            write: false,
        }];
        assert!(resolve_grant_abs(&read_only, &root.join("sub"), true).is_err());
        assert!(resolve_grant_abs(&read_only, &root.join("sub"), false).is_ok());
    }

    #[test]
    fn resolve_grant_abs_rejects_symlink_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("granted");
        std::fs::create_dir_all(&root).expect("mkdir");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("mkdir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

        let grants = vec![FsGrant {
            slot: "workspace".into(),
            path: root.canonicalize().expect("canonical"),
            read: true,
            write: false,
        }];
        // The symlink canonicalizes to the outside directory: containment
        // fails even though the link itself lives inside the grant.
        assert!(resolve_grant_abs(&grants, &root.join("link"), false).is_err());
    }
}
