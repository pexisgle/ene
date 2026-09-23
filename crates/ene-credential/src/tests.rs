use crate::CredentialTechnicalError;
use crate::auth_file::FileDeviceAuthStore;
use crate::pairing::DeviceId;
use crate::registry::{
    CredentialRef, CredentialRefError, CredentialRefRepository, available_credential,
};
use crate::secret::{
    CredentialStore, MemoryCredentialStore, MemoryVersionedStore, VersionedCredentialStore,
};
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
    store
        .put(&cred, "rotated-bearer")
        .expect("memory store accepts serving-time put");
    let rotated = store.with_bearer(&cred, str::to_owned).expect("put bearer");
    assert_eq!(rotated, "rotated-bearer");
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
fn active_version_is_an_immutable_snapshot_not_a_fresh_backend_read() {
    let store = MemoryVersionedStore::new();
    let cred = acme_main();
    store
        .put_version(&cred, 41, "published-secret")
        .expect("candidate write");
    let snapshot = store
        .prepare_snapshot(&cred, 41)
        .expect("snapshot preparation");
    store.activate(snapshot);
    store
        .delete_version(&cred, 41)
        .expect("simulate an external backend change");
    assert_eq!(
        store.with_bearer(&cred, str::to_owned),
        Ok(String::from("published-secret")),
        "routine use must retain the immutable published snapshot for its revision"
    );
}

#[test]
fn failed_snapshot_publication_does_not_fall_back_to_the_old_version() {
    let store = MemoryVersionedStore::new();
    let cred = acme_main();
    store
        .put_version(&cred, 41, "old-secret")
        .expect("old candidate write");
    let old = store
        .prepare_snapshot(&cred, 41)
        .expect("old snapshot preparation");
    store.activate(old);

    assert!(
        store.prepare_snapshot(&cred, 42).is_err(),
        "a missing committed version must fail preparation"
    );
    store.deactivate(&cred);
    assert!(
        store.with_bearer(&cred, str::to_owned).is_err(),
        "publication failure must clear the old snapshot instead of falling back"
    );
}

#[test]
fn pairing_proof_conforms_to_rfc4231_and_rejects_mismatch() {
    let key = "\x0b".repeat(20);
    let proof = crate::pairing::pairing_proof_hex(&key, "Hi There");
    assert_eq!(
        proof.as_str(),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert!(crate::pairing::verify_pairing_proof(
        &key, "Hi There", &proof
    ));

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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
    let loaded = store.load_secret(&device);
    let secret = loaded.unwrap().unwrap();
    assert_eq!(secret.bytes(), "pairing-secret-value".as_bytes());
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

#[test]
fn device_auth_concurrent_approvals_keep_both_devices() {
    use std::sync::{Arc, Barrier};

    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = Arc::new(open_device_auth_store(&path));
    let first = DeviceId(RawId::new());
    let second = DeviceId(RawId::new());
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for (device, descriptor) in [(first, "phone"), (second, "tablet")] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            store.save_secret(&device, descriptor, "shared-race-secret")
        }));
    }
    barrier.wait();
    for thread in threads {
        assert!(
            thread.join().expect("writer must not panic").is_ok(),
            "a concurrent approval must persist"
        );
    }
    assert!(
        store.load_secret(&first).unwrap().is_some(),
        "the first concurrent approval must survive"
    );
    assert!(
        store.load_secret(&second).unwrap().is_some(),
        "the second concurrent approval must survive"
    );
}

#[test]
fn device_auth_same_device_rotations_leave_one_current_secret() {
    use std::sync::{Arc, Barrier};

    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = Arc::new(open_device_auth_store(&path));
    let device = DeviceId(RawId::new());
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for secret in ["first-secret", "second-secret"] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            store.save_secret(&device, "phone", secret)
        }));
    }
    barrier.wait();
    for thread in threads {
        assert!(
            thread.join().expect("writer must not panic").is_ok(),
            "a concurrent rotation must persist"
        );
    }
    let loaded = store.load_secret(&device);
    let current = loaded.unwrap().unwrap();
    assert!(
        current.bytes() == b"first-secret" || current.bytes() == b"second-secret",
        "exactly one written secret stays current"
    );
    let raw = std::fs::read(&path).unwrap_or_default();
    let document: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        document["devices"].as_object().map(serde_json::Map::len),
        Some(1),
        "one device holds exactly one entry regardless of the race"
    );
}

/// The mutation lock is a real cross-handle exclusion: a second writer waits
/// until the holder releases, and dropping the holder recovers the file for
/// the next approval (the kernel owns release-on-exit, so a crash behaves
/// like a drop).
#[test]
fn device_auth_mutation_lock_serializes_writers() {
    use std::sync::mpsc;
    use std::time::Duration;

    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder_path = path.clone();
    let holder = std::thread::spawn(move || {
        let store = open_device_auth_store(&holder_path);
        store.with_mutation_lock(|| {
            acquired_tx.send(()).expect("test receiver lives");
            release_rx.recv().expect("test sender lives");
            Ok(())
        })
    });
    acquired_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the holder must acquire the mutation lock");

    let (done_tx, done_rx) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let store = open_device_auth_store(&writer_path);
        let saved = store.save_secret(&DeviceId(RawId::new()), "phone", "racing");
        done_tx.send(saved.is_ok()).expect("test receiver lives");
    });
    assert!(
        done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "a second writer must wait for the held lock"
    );
    release_tx.send(()).expect("holder waits for release");
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the writer completes after the release"),
        "the released lock lets the writer persist"
    );
    holder.join().expect("holder must not panic").unwrap();
    writer.join().expect("writer must not panic");
}

/// A failed approval leaves the previous custody document intact, and the
/// next approval recovers once the cause (here: a malformed file) is
/// removed.
#[test]
fn device_auth_write_failure_recovers_on_the_next_approval() {
    let temp = fresh_tempdir();
    let path = temp.path().join("device-auth.json");
    let store = open_device_auth_store(&path);
    let written = std::fs::write(&path, b"not json{{{");
    assert!(written.is_ok(), "the malformed fixture must write");
    let device = DeviceId(RawId::new());
    assert!(
        store.save_secret(&device, "phone", "first-try").is_err(),
        "a malformed file refuses the write, never clobbers"
    );
    assert!(
        std::fs::read(&path).unwrap_or_default() == b"not json{{{",
        "the failed approval must not rewrite the document"
    );
    std::fs::remove_file(&path).expect("the malformed fixture must be removable");
    assert!(
        store.save_secret(&device, "phone", "second-try").is_ok(),
        "the next approval recovers after the cause is cleared"
    );
    let loaded = store.load_secret(&device);
    assert_eq!(loaded.unwrap().unwrap().bytes(), b"second-try");
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

#[test]
fn memory_put_stores_without_echoing_the_secret_in_debug() {
    let store = MemoryCredentialStore::new();
    let credential = CredentialRef::new("openai", "rotated").expect("valid test fixture");
    store
        .put(&credential, "sk-must-not-appear")
        .expect("memory put must accept");
    assert!(store.contains(&credential));
    let rendered = format!("{store:?}");
    assert!(
        !rendered.contains("sk-must-not-appear"),
        "memory store Debug must not show the secret: {rendered}"
    );
}

mod env_credential_store_tests {
    use crate::registry::CredentialRef;
    use crate::secret::{CredentialStore, ENV_API_KEY, EnvCredentialStore, resolve_for};
    use std::cell::Cell;

    #[test]
    fn lookup_respects_provider_gating_and_presence() {
        let calls = Cell::new(0_u32);
        let resolved = resolve_for("acme", |_| {
            calls.set(calls.get() + 1);
            Some("test-key".to_owned())
        });
        assert!(resolved.is_none());
        assert_eq!(calls.get(), 0);

        let resolved = resolve_for("openai", |name| {
            assert_eq!(name, ENV_API_KEY);
            Some("test-key".to_owned())
        });
        assert_eq!(resolved.as_deref(), Some("test-key"));

        let resolved = resolve_for("openai", |_| None);
        assert!(resolved.is_none());
    }

    #[test]
    fn put_fail_closes_and_never_echoes_the_secret() {
        let store = EnvCredentialStore::from_lookup(|_| Some("pinned-key".to_owned()));
        let credential = CredentialRef::new("openai", "main").expect("valid test fixture");
        let error = store
            .put(&credential, "sk-must-not-appear")
            .expect_err("env store is not a product source of truth");
        let rendered = error.to_string();
        assert!(
            !rendered.contains("sk-must-not-appear"),
            "env put error must not echo the secret: {rendered}"
        );
        assert!(
            rendered.contains("not a product source of truth"),
            "env put must fail closed: {rendered}"
        );
    }
}
