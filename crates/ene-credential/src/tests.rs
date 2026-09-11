use crate::CredentialTechnicalError;
use crate::auth_file::FileDeviceAuthStore;
use crate::pairing::DeviceId;
use crate::registry::{
    CredentialRef, CredentialRefError, CredentialRefRepository, available_credential,
};
use crate::secret::{CredentialStore, MemoryCredentialStore};
use ene_primitive::RawId;

struct FakeRepo(Vec<CredentialRef>);

impl CredentialRefRepository for FakeRepo {
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        Ok(self.0.clone())
    }
}

fn acme_main() -> CredentialRef {
    CredentialRef::new("acme", "main").expect("valid test fixture")
}

#[tokio::test]
async fn availability_requires_both_registry_and_store() {
    let store = MemoryCredentialStore::new();
    let cred = acme_main();
    let registry = FakeRepo(vec![cred.clone()]);
    let missing = available_credential("acme", &cred.id(), &registry, &store).await;
    assert_eq!(missing, Ok(None), "a ref without a bearer is unavailable");
    let absent_registry = FakeRepo(Vec::new());
    store.insert(cred.clone(), "bearer-token");
    let store_only = available_credential("acme", &cred.id(), &absent_registry, &store).await;
    assert_eq!(
        store_only,
        Ok(None),
        "a bearer without a usable ref is unavailable"
    );
    let both = available_credential("acme", &cred.id(), &registry, &store).await;
    assert_eq!(both, Ok(Some(cred.clone())), "both sides must agree");
    let wrong_provider = available_credential("other", &cred.id(), &registry, &store).await;
    assert_eq!(
        wrong_provider,
        Ok(None),
        "a different provider never resolves"
    );
}

#[test]
fn credential_ref_grammar_is_fixed() {
    assert_eq!(acme_main().id(), "acme:main");
    assert_eq!(acme_main().provider(), "acme");
    assert_eq!(acme_main().label(), "main");
    let colon_label = CredentialRef::new("acme", "team:main").expect("label may contain ':'");
    assert_eq!(colon_label.id(), "acme:team:main");
    assert_eq!(colon_label.label(), "team:main");
    assert_eq!(
        CredentialRef::new("acme:bad", "main"),
        Err(CredentialRefError::InvalidProvider)
    );
    assert_eq!(
        CredentialRef::new(" ", "main"),
        Err(CredentialRefError::InvalidProvider)
    );
    assert_eq!(
        CredentialRef::new("acme", ""),
        Err(CredentialRefError::InvalidLabel)
    );
}

#[test]
fn bearer_closure_receives_the_inserted_secret() {
    let store = MemoryCredentialStore::new();
    let cred = acme_main();
    store.insert(cred.clone(), "bearer-token");
    let seen = store.with_bearer(&cred, str::len);
    assert_eq!(seen, Ok("bearer-token".len()));
}

#[test]
fn pairing_proof_matches_rfc4231_case_1() {
    let key = "\x0b".repeat(20);
    let proof = crate::pairing::pairing_proof_hex(&key, "Hi There");
    assert_eq!(
        proof.as_str(),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert!(crate::pairing::verify_pairing_proof(
        &key, "Hi There", &proof
    ));
}

#[test]
fn pairing_proof_round_trips_and_rejects_mismatch() {
    let proof = crate::pairing::pairing_proof_hex("pairing-secret", "single-use-nonce");
    assert!(crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "single-use-nonce",
        &proof
    ));
    assert!(!crate::pairing::verify_pairing_proof(
        "other-secret",
        "single-use-nonce",
        &proof
    ));
    assert!(!crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "other-nonce",
        &proof
    ));
    assert!(!crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "single-use-nonce",
        "not-hex!!"
    ));
    assert!(!crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "single-use-nonce",
        ""
    ));
    assert!(!crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "single-use-nonce",
        "B0344C61D8DB38535CA8AFCEAF0BF12B881DC200C9833DA726E9376C2E32CFF7"
    ));
}

#[test]
fn pairing_proof_rejects_tampered_hex() {
    let proof = crate::pairing::pairing_proof_hex("pairing-secret", "single-use-nonce");
    let tampered: String = proof
        .chars()
        .enumerate()
        .map(|(index, digit)| {
            if index == 0 {
                if digit == '0' { '1' } else { '0' }
            } else {
                digit
            }
        })
        .collect();
    assert_ne!(tampered, proof);
    assert!(!crate::pairing::verify_pairing_proof(
        "pairing-secret",
        "single-use-nonce",
        &tampered
    ));
}

fn fresh_tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir must be available")
}

fn open_device_auth_store(path: &std::path::Path) -> FileDeviceAuthStore {
    FileDeviceAuthStore::open(path).expect("device-auth store must open")
}

#[test]
fn device_auth_roundtrip_preserves_secret_bytes() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let device = DeviceId(RawId::new());
    let saved = store.save_secret(&device, "phone", "pairing-secret-value");
    assert!(saved.is_ok(), "save must succeed");
    let loaded = store.load_secret(&device);
    let secret = loaded.unwrap().unwrap();
    assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
}

#[test]
fn device_auth_missing_file_loads_none_and_missing_parent_fails_open() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let loaded = store.load_secret(&DeviceId(RawId::new()));
    assert!(matches!(loaded, Ok(None)));
    let nested = temp.path().join("no-such-dir").join("device-auth.json");
    assert!(FileDeviceAuthStore::open(&nested).is_err());
}

#[test]
fn device_auth_malformed_files_error_never_default() {
    let entry = "{\"secret_hex\":\"00\",\"descriptor\":\"d\",\
         \"paired_at\":\"2026-09-08T12:00:00+09:00\"}";
    let key_prefix = "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":";
    let keyed = key_prefix.to_owned() + entry + "}}";
    let bad_key = "{\"devices\":{\"not-a-uuid\":".to_owned() + entry + "}}";
    let extra_field = key_prefix.to_owned() + entry + ",\"extra\":\"x\"}}";
    let duplicate_key =
        key_prefix.to_owned() + entry + ",\"123e4567-e89b-12d3-a456-426614174000\":" + entry + "}}";
    let fixtures = [
        "not json{{{".to_owned(),
        "[]".to_owned(),
        "{}".to_owned(),
        "{\"other\":{}}".to_owned(),
        "{\"devices\":[]}".to_owned(),
        "{\"devices\":{}}trailing".to_owned(),
        bad_key,
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"zz\",\"descriptor\":\"d\",\
         \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
            .to_owned(),
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"AABB\",\"descriptor\":\"d\",\
         \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
            .to_owned(),
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"00\",\"descriptor\":\"d\",\"paired_at\":\"yesterday\"}}}"
            .to_owned(),
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"00\",\"descriptor\":\"d\"}}}"
            .to_owned(),
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"00\",\"descriptor\":\"d\",\
         \"paired_at\":\"2026-09-08T12:00:00+09:00\",\"unknown\":\"x\"}}}"
            .to_owned(),
        "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
         {\"secret_hex\":\"00\",\"secret_hex\":\"00\",\"descriptor\":\"d\",\
         \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}"
            .to_owned(),
        extra_field,
        keyed.clone() + "}]",
        duplicate_key,
    ];
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    for fixture in fixtures {
        let written = std::fs::write(&path, &fixture);
        assert!(written.is_ok(), "fixture setup must succeed");
        let store = open_device_auth_store(&path);
        let device = DeviceId(RawId::new());
        assert!(
            store.load_secret(&device).is_err(),
            "malformed file must error, never default"
        );
        assert!(
            store.save_secret(&device, "phone", "secret").is_err(),
            "saving over a malformed file must error, never clobber"
        );
    }
}

#[test]
fn device_auth_file_renders_canonical_json() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let first = DeviceId(RawId::new());
    let second = DeviceId(RawId::new());
    assert!(store.save_secret(&first, "phone", "first-secret").is_ok());
    assert!(
        store
            .save_secret(&second, "tablet", "second-secret")
            .is_ok()
    );
    let raw = std::fs::read(&path);
    let raw = raw.unwrap();
    assert!(
        raw.starts_with(b"{\"devices\":{"),
        "rendering keeps the single-section shape"
    );
    assert!(
        raw.ends_with(b"}}\n"),
        "rendering is compact with a trailing newline"
    );
    assert!(
        !raw.contains(&b' '),
        "rendering carries no whitespace padding"
    );
}

#[test]
fn device_auth_reads_pre_serde_documents() {
    // Same document shape as earlier releases (field order and escape
    // sequences): existing custody files must keep parsing.
    let fixture = "{\"devices\":{\"123e4567-e89b-12d3-a456-426614174000\":\
        {\"secret_hex\":\"00\",\"descriptor\":\"a\\\"b\\\\nc✓\",\
        \"paired_at\":\"2026-09-08T12:00:00+09:00\"}}}\n";
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let written = std::fs::write(&path, fixture);
    assert!(written.is_ok(), "fixture setup must succeed");
    let store = open_device_auth_store(&path);
    let device =
        crate::auth_file::parse_device_key("123e4567-e89b-12d3-a456-426614174000").unwrap();
    let loaded = store.load_secret(&device);
    let secret = loaded.unwrap().unwrap();
    assert_eq!(secret.bytes(), &[0x00]);
}

#[cfg(unix)]
#[test]
fn device_auth_open_tightens_lax_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let written = std::fs::write(&path, "{\"devices\":{}}");
    assert!(written.is_ok(), "fixture setup must succeed");
    let lax = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
    assert!(lax.is_ok(), "fixture setup must succeed");
    let store = open_device_auth_store(&path);
    let meta = std::fs::metadata(&path);
    let meta = meta.unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let loaded = store.load_secret(&DeviceId(RawId::new()));
    assert!(matches!(loaded, Ok(None)));
}

#[cfg(unix)]
#[test]
fn device_auth_saved_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let saved = store.save_secret(&DeviceId(RawId::new()), "phone", "pairing-secret");
    assert!(saved.is_ok(), "save must succeed");
    let meta = std::fs::metadata(&path);
    let meta = meta.unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn device_auth_second_save_rotates_the_secret() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let device = DeviceId(RawId::new());
    assert!(store.save_secret(&device, "phone", "first-secret").is_ok());
    assert!(store.save_secret(&device, "phone", "second-secret").is_ok());
    let loaded = store.load_secret(&device);
    let secret = loaded.unwrap().unwrap();
    assert_eq!(secret.bytes(), "second-secret".as_bytes());
}

#[test]
fn device_auth_persists_across_store_instances() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let device = DeviceId(RawId::new());
    let descriptor = "phone \"pro\"\nline2\t✓";
    let first = open_device_auth_store(&path);
    assert!(
        first
            .save_secret(&device, descriptor, "pairing-secret-value")
            .is_ok()
    );
    drop(first);
    let second = open_device_auth_store(&path);
    let loaded = second.load_secret(&device);
    let secret = loaded.unwrap().unwrap();
    assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
}

#[test]
fn device_auth_debug_carries_no_secret_or_descriptor() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let marker = "marker-secret-9d3f41";
    let descriptor = "marker-descriptor-6be2";
    assert!(
        store
            .save_secret(&DeviceId(RawId::new()), descriptor, marker)
            .is_ok()
    );
    let rendered = format!("{store:?}");
    let marker_hex = crate::pairing::encode_hex_lower(marker.as_bytes());
    assert!(rendered.contains("FileDeviceAuthStore"));
    assert!(rendered.contains("entries"));
    assert!(!rendered.contains(marker));
    assert!(!rendered.contains(marker_hex.as_str()));
    assert!(!rendered.contains(descriptor));
}

mod env_credential_store_tests {
    use crate::CredentialTechnicalError;
    use crate::registry::CredentialRef;
    use crate::secret::{CredentialStore, ENV_API_KEY, EnvCredentialStore, resolve_for};
    use std::cell::Cell;

    fn other_cred() -> CredentialRef {
        CredentialRef::new("acme", "main").expect("valid test fixture")
    }

    #[test]
    fn lookup_gates_on_provider_before_reading_env() {
        let calls = Cell::new(0_u32);
        let resolved = resolve_for("acme", |_| {
            calls.set(calls.get() + 1);
            Some("test-key".to_owned())
        });
        assert!(resolved.is_none());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn lookup_accepts_a_present_non_empty_value() {
        let resolved = resolve_for("openai", |name| {
            assert_eq!(name, ENV_API_KEY);
            Some("test-key".to_owned())
        });
        assert_eq!(resolved.as_deref(), Some("test-key"));
    }

    #[test]
    fn lookup_treats_a_missing_value_as_absent() {
        let resolved = resolve_for("openai", |_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn lookup_treats_an_empty_value_as_absent() {
        let resolved = resolve_for("openai", |_| Some(String::new()));
        assert!(resolved.is_none());
    }

    #[test]
    fn store_reports_other_providers_absent_without_re_reading_env() {
        let calls = Cell::new(0_u32);
        let store = EnvCredentialStore::from_lookup(|_| {
            calls.set(calls.get() + 1);
            Some("test-key".to_owned())
        });
        assert_eq!(calls.get(), 1, "construction pins the value once");
        assert!(!store.contains(&other_cred()));
        assert_eq!(calls.get(), 1, "other providers never re-read the source");
    }

    #[test]
    fn store_with_bearer_rejects_other_providers() {
        let store = EnvCredentialStore::from_lookup(|_| Some("test-key".to_owned()));
        let outcome = store.with_bearer(&other_cred(), str::len);
        let CredentialTechnicalError::StorageUnavailable { reason } = outcome.unwrap_err();
        assert_eq!(reason, "env credential missing");
    }

    #[test]
    fn store_pins_the_value_at_construction() {
        let calls = Cell::new(0_u32);
        let store = EnvCredentialStore::from_lookup(|name| {
            assert_eq!(name, ENV_API_KEY);
            calls.set(calls.get() + 1);
            Some("pinned-key".to_owned())
        });
        assert_eq!(calls.get(), 1, "construction reads the source once");
        let credential = CredentialRef::new("openai", "main").expect("valid test fixture");
        let first = store.with_bearer(&credential, str::to_owned).unwrap();
        let second = store.with_bearer(&credential, str::to_owned).unwrap();
        assert_eq!(first, "pinned-key");
        assert_eq!(second, "pinned-key");
        assert_eq!(calls.get(), 1, "calls never re-read the source");
    }
}
