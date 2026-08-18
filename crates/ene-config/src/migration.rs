//! Config-version migration for `settings.json`.
//!
//! [`EneConfig`](crate::EneConfig) carries a `version` field. When the on-disk
//! schema changes, registered steps rewrite the *raw JSON* of an old
//! `settings.json` forward, one version at a time, before the figment pipeline
//! deserialises it into the typed [`EneConfig`](crate::EneConfig).
//!
//! # Why raw JSON, not the typed struct
//!
//! Migrations run on [`serde_json::Value`] rather than on
//! [`EneConfig`](crate::EneConfig) because a schema change may alter a field's
//! *type*. Rewriting the JSON first sidesteps deserialise failures.
//!
//! # How steps are registered and run
//!
//! A step is a `fn(&mut serde_json::Value) -> Result<(), EneConfigError>`
//! registered against the version it migrates *from*: a step registered for
//! version `N` rewrites a version-`N` document into a version-`N+1` document.
//! [`apply_migrations`] runs the steps for `from`, `from+1`, … in ascending
//! order until the document reaches [`CURRENT_CONFIG_VERSION`], stamping the
//! `version` field after each step.
//!
//! # Version policy
//!
//! * A file **older** than [`CURRENT_CONFIG_VERSION`] is migrated forward and
//!   the new version is persisted by the caller.
//! * A file **at** the current version is returned unchanged.
//! * A file **newer** than the current version is
//!   [`EneConfigError::ConfigVersionTooNew`]. The file is left untouched.
//!
//! # Adding a real migration
//!
//! 1. Bump [`CURRENT_CONFIG_VERSION`].
//! 2. Write a step that rewrites a version-`(N-1)` document into version `N`.
//! 3. Register it with [`register_migration`] for `from = N - 1`.
//!
//! Version 8 drops the retired `plugins.list` map and leftover engine keys
//! (`ai.local_models`, `ai.ort_dylib_path`). Provider binding is
//! `ai.tasks.*`; MCP rows live in `mcp.json`. Values are not copied into a
//! new home. Steps 1–7 are the same drop so any older document reaches 8.

use crate::error::EneConfigError;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The schema version the current build reads and writes.
///
/// A `settings.json` whose `version` is below this is migrated forward on load;
/// one whose `version` exceeds it is rejected (see
/// [`EneConfigError::ConfigVersionTooNew`]). Bump this whenever you register a
/// new migration step.
pub const CURRENT_CONFIG_VERSION: u32 = 8;

const VERSION_KEY: &str = "version";

/// A single migration step.
///
/// Receives the whole settings document as raw JSON and rewrites it in place
/// from version `N` to version `N + 1`, where `N` is the version the step was
/// registered against. The `version` field itself is stamped by
/// [`apply_migrations`]; steps should focus on the fields whose shape changed.
pub type MigrationFn = fn(&mut serde_json::Value) -> Result<(), EneConfigError>;

/// Process-wide registry of migration steps, keyed by the version they migrate
/// from.
///
/// A `parking_lot::Mutex` is used (it never poisons on panic), matching the
/// lock strategy elsewhere in this crate.
static MIGRATIONS: OnceLock<parking_lot::Mutex<HashMap<u32, MigrationFn>>> = OnceLock::new();

/// Set once [`apply_migrations`] has run for a document.
///
/// Registration after the first application is a programming error (the
/// migration can no longer have any effect for this process), so
/// [`register_migration`] rejects it loudly instead of silently storing a
/// step that will never run. This makes registration-order bugs detectable
/// at startup rather than on a later load.
static APPLIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Test-only override for the current version, so multi-step migration chains
/// can be exercised without bumping the real [`CURRENT_CONFIG_VERSION`].
/// A value of `0` means "no override; use the constant".
#[cfg(test)]
static TEST_VERSION_OVERRIDE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Serialises every test that mutates the process-global migration state (the
/// version override and the registry). Crate-visible so the `config.rs`
/// load-path tests can share the same lock.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn registry() -> &'static parking_lot::Mutex<HashMap<u32, MigrationFn>> {
    MIGRATIONS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// Returns the effective current schema version.
///
/// In production this is always [`CURRENT_CONFIG_VERSION`]. Under `cfg(test)`
/// an override may be installed via `TEST_VERSION_OVERRIDE` to drive the
/// multi-version tests below.
fn current_version() -> u32 {
    #[cfg(test)]
    {
        let overridden = TEST_VERSION_OVERRIDE.load(std::sync::atomic::Ordering::Acquire);
        if overridden != 0 {
            return overridden;
        }
    }
    CURRENT_CONFIG_VERSION
}

/// Registers a migration step that rewrites a version-`from` document into
/// version `from + 1`.
///
/// Registering a second step for the same `from` version replaces the previous
/// one, so startup registration is idempotent.
///
/// # Errors
///
/// Returns [`EneConfigError::GenericConfigError`] for programming errors that
/// would otherwise be silently swallowed:
///
/// * registering a step for `from >= CURRENT_CONFIG_VERSION` — there is no
///   version to migrate *to*; and
/// * registering a step after [`apply_migrations`] has already run in this
///   process — the step could never take effect.
///
/// Callers (typically `ctor` initialisers in schema-owning crates) should
/// propagate or log the error at startup, where it is still visible.
pub fn register_migration(from: u32, step: MigrationFn) -> Result<(), EneConfigError> {
    if from >= current_version() {
        return Err(EneConfigError::GenericConfigError(format!(
            "cannot register config migration from version {from}: \
             no version to migrate to (current is {})",
            current_version()
        )));
    }
    if APPLIED.load(std::sync::atomic::Ordering::Acquire) {
        return Err(EneConfigError::GenericConfigError(format!(
            "cannot register config migration from version {from} after \
             migrations have already been applied in this process"
        )));
    }
    registry().lock().insert(from, step);
    Ok(())
}

/// Reads the `version` field out of a raw settings document.
///
/// A missing `version` is treated as `1`: the earliest shipped schema was
/// version 1, and very early hand-written files predate the field. A present
/// but non-numeric `version` is an error rather than a silent default, since it
/// signals a corrupt file.
fn read_version(doc: &serde_json::Value) -> Result<u32, EneConfigError> {
    match doc.get(VERSION_KEY) {
        None | Some(serde_json::Value::Null) => Ok(1),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                EneConfigError::GenericConfigError(format!(
                    "config `{VERSION_KEY}` is not a valid u32: {n}"
                ))
            }),
        Some(other) => Err(EneConfigError::GenericConfigError(format!(
            "config `{VERSION_KEY}` must be a number, found {other}"
        ))),
    }
}

/// The schema version a raw settings document declares, for callers that need
/// it before [`apply_migrations`] consumes the document.
///
/// A document whose `version` is missing or malformed reports `1`, matching
/// [`read_version`]'s treatment of pre-versioning files; callers use this only
/// to label artifacts (such as the pre-migration backup), so a corrupt value
/// must not fail on its own — `apply_migrations` reports it properly.
pub(crate) fn document_version(doc: &serde_json::Value) -> u32 {
    read_version(doc).unwrap_or(1)
}

fn set_version(doc: &mut serde_json::Value, version: u32) -> Result<(), EneConfigError> {
    if doc.is_null() {
        *doc = serde_json::json!({});
    }
    let object = doc.as_object_mut().ok_or_else(|| {
        EneConfigError::GenericConfigError(
            "config document is not a JSON object; cannot stamp version".to_string(),
        )
    })?;
    object.insert(VERSION_KEY.to_string(), serde_json::Value::from(version));
    Ok(())
}

/// Migrates a raw settings document forward to the current schema version.
///
/// Returns the (possibly rewritten) document with its `version` field stamped
/// to the current version. If the document is already current its fields are
/// left unchanged (the `version` field is still normalised to an explicit
/// current value); if it is newer, an error is returned and the document is
/// left untouched.
///
/// This operates on raw JSON — not the typed [`EneConfig`](crate::EneConfig) —
/// so it can repair documents that would no longer deserialise into the current
/// struct. See the [module docs](self) for the rationale.
///
/// # Errors
///
/// Returns [`EneConfigError::ConfigVersionTooNew`] if the document's version
/// exceeds the current version, [`EneConfigError::GenericConfigError`] if the
/// version field is malformed or a required migration step is missing, or any
/// error propagated from a migration step itself.
pub fn apply_migrations(mut doc: serde_json::Value) -> Result<serde_json::Value, EneConfigError> {
    let target = current_version();
    let mut version = read_version(&doc)?;

    if version > target {
        return Err(EneConfigError::ConfigVersionTooNew {
            found: version,
            supported: target,
        });
    }

    while version < target {
        let Some(step) = registry().lock().get(&version).copied() else {
            return Err(EneConfigError::GenericConfigError(format!(
                "no migration registered to move config from version {version} to {}; \
                 cannot migrate settings.json forward",
                version.saturating_add(1)
            )));
        };
        step(&mut doc)?;
        version = version.saturating_add(1);
        set_version(&mut doc, version)?;
        tracing::info!(
            component = "Config",
            version,
            "applied config migration step"
        );
    }

    // From here on, any migration registered later in this process could
    // never run for a document that has already been migrated — record that
    // so register_migration can reject late registration loudly.
    APPLIED.store(true, std::sync::atomic::Ordering::Release);

    // Guarantee the invariant that a migrated document always carries an
    // explicit, current `version` field — even when no steps ran (e.g. a file
    // that omits the field entirely, treated as version 1). Idempotent with the
    // per-step stamping above.
    set_version(&mut doc, target)?;

    Ok(doc)
}

/// Drops retired plugin-list and in-process engine keys. Does not map them
/// onto `ai.tasks.*` or profile rows.
pub(crate) fn migrate_drop_legacy_plugin_list(
    doc: &mut serde_json::Value,
) -> Result<(), EneConfigError> {
    let Some(root) = doc.as_object_mut() else {
        return Ok(());
    };
    if let Some(plugins) = root
        .get_mut("plugins")
        .and_then(serde_json::Value::as_object_mut)
    {
        plugins.remove("list");
        plugins.remove("enabled");
    }
    if let Some(ai) = root
        .get_mut("ai")
        .and_then(serde_json::Value::as_object_mut)
    {
        ai.remove("local_models");
        ai.remove("ort_dylib_path");
    }
    Ok(())
}

/// Registers drop steps for every version below [`CURRENT_CONFIG_VERSION`].
const _: () = {
    /// # Safety
    ///
    /// Called by `ctor` before `main`. Only safe registration code
    /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
    #[ene_config::ctor(unsafe, crate_path = ene_config)]
    fn register_drop_legacy_plugin_list() {
        for from in 1..CURRENT_CONFIG_VERSION {
            if let Err(err) = register_migration(from, migrate_drop_legacy_plugin_list) {
                tracing::error!(
                    component = "Config",
                    error = %err,
                    from,
                    "failed to register settings.json migration"
                );
            }
        }
    }
};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Runs `body` with the effective current version temporarily set to
    /// `version` and the registry cleared, restoring both afterwards.
    ///
    /// All tests that touch the process-global migration state route through
    /// here (or acquire [`TEST_LOCK`] directly), because the override and the
    /// registry are shared across the whole process.
    pub(crate) fn with_test_version<F: FnOnce()>(version: u32, body: F) {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Snapshot the registry so it can be restored afterwards: tests run in
        // arbitrary order, and a test that clears the registry must not leave
        // the process without the production migration steps for the next test.
        let saved_registry = registry().lock().clone();
        let saved_applied = APPLIED.load(std::sync::atomic::Ordering::Acquire);

        TEST_VERSION_OVERRIDE.store(version, std::sync::atomic::Ordering::Release);
        registry().lock().clear();
        APPLIED.store(false, std::sync::atomic::Ordering::Release);

        body();

        TEST_VERSION_OVERRIDE.store(0, std::sync::atomic::Ordering::Release);
        *registry().lock() = saved_registry;
        APPLIED.store(saved_applied, std::sync::atomic::Ordering::Release);
    }

    #[test]
    fn current_version_is_untouched() {
        with_test_version(1, || {
            let doc = serde_json::json!({
                "version": 1,
                "character": "Alicia",
                "user_name": "User",
            });
            let migrated = apply_migrations(doc.clone()).expect("current version migrates ok");
            assert_eq!(migrated, doc, "current-version config must be unchanged");
        });
    }

    #[test]
    fn missing_version_defaults_to_one() {
        with_test_version(1, || {
            let doc = serde_json::json!({ "character": "Alicia" });
            let migrated = apply_migrations(doc).expect("missing version migrates ok");
            assert_eq!(
                migrated.get("version"),
                Some(&serde_json::json!(1)),
                "missing version should be stamped as 1"
            );
        });
    }

    #[test]
    fn newer_version_errors() {
        with_test_version(1, || {
            let doc = serde_json::json!({ "version": 99, "character": "Alicia" });
            let err = apply_migrations(doc).expect_err("newer version must error");
            assert!(
                matches!(
                    err,
                    EneConfigError::ConfigVersionTooNew {
                        found: 99,
                        supported: 1,
                    }
                ),
                "expected ConfigVersionTooNew, got {err:?}"
            );
        });
    }

    #[test]
    fn old_version_is_migrated_to_current() {
        with_test_version(2, || {
            register_migration(1, |doc| {
                if let Some(obj) = doc.as_object_mut()
                    && let Some(name) = obj.remove("name")
                {
                    obj.insert("user_name".to_string(), name);
                }
                Ok(())
            })
            .expect("registration below current version succeeds");

            let doc = serde_json::json!({ "version": 1, "name": "Hoshino" });
            let migrated = apply_migrations(doc).expect("old version migrates ok");

            assert_eq!(migrated.get("version"), Some(&serde_json::json!(2)));
            assert_eq!(
                migrated.get("user_name"),
                Some(&serde_json::json!("Hoshino"))
            );
            assert!(migrated.get("name").is_none(), "old field should be gone");
        });
    }

    /// Appends the document's current `version` to a `trail` array.
    ///
    /// Registered for several source versions below; because a step runs before
    /// [`apply_migrations`] stamps the next version, `doc["version"]` still
    /// holds the step's source version, so the trail records execution order
    /// without the step needing to capture any state (a `MigrationFn` is a bare
    /// `fn` pointer and cannot close over variables).
    fn trail_step(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
        let obj = doc
            .as_object_mut()
            .ok_or_else(|| EneConfigError::GenericConfigError("not an object".to_string()))?;
        let current = obj
            .get(VERSION_KEY)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let trail = obj.entry("trail").or_insert_with(|| serde_json::json!([]));
        trail
            .as_array_mut()
            .ok_or_else(|| EneConfigError::GenericConfigError("not an array".to_string()))?
            .push(serde_json::json!(current));
        Ok(())
    }

    #[test]
    fn migrations_run_in_order() {
        with_test_version(4, || {
            for from in 1..4 {
                register_migration(from, trail_step)
                    .expect("registration below current version succeeds");
            }

            let doc = serde_json::json!({ "version": 1 });
            let migrated = apply_migrations(doc).expect("multi-step migration ok");

            assert_eq!(migrated.get("version"), Some(&serde_json::json!(4)));
            assert_eq!(
                migrated.get("trail"),
                Some(&serde_json::json!([1, 2, 3])),
                "steps must run in ascending version order"
            );
        });
    }

    #[test]
    fn missing_intermediate_step_errors() {
        with_test_version(3, || {
            register_migration(1, |_| Ok(())).expect("registration below current version succeeds");

            let doc = serde_json::json!({ "version": 1 });
            let err = apply_migrations(doc).expect_err("gap in chain must error");
            assert!(
                matches!(err, EneConfigError::GenericConfigError(_)),
                "expected GenericConfigError, got {err:?}"
            );
        });
    }

    #[test]
    fn non_numeric_version_errors() {
        with_test_version(1, || {
            let doc = serde_json::json!({ "version": "one" });
            let err = apply_migrations(doc).expect_err("string version must error");
            assert!(
                matches!(err, EneConfigError::GenericConfigError(_)),
                "expected GenericConfigError, got {err:?}"
            );
        });
    }

    #[test]
    fn register_at_or_above_current_is_rejected() {
        with_test_version(2, || {
            register_migration(1, |_| Ok(())).expect("registration below current version succeeds");
            register_migration(2, |_| Ok(()))
                .expect_err("registration at current version must error");
            register_migration(5, |_| Ok(()))
                .expect_err("registration above current version must error");
            let reg = registry().lock();
            assert!(
                reg.get(&1).is_some(),
                "a step below the current version must be stored"
            );
            assert!(
                reg.get(&2).is_none() && reg.get(&5).is_none(),
                "steps at or above the current version must not be stored"
            );
        });
    }

    #[test]
    fn drop_legacy_plugin_list_does_not_relocate() {
        let mut doc = serde_json::json!({
            "version": 7,
            "plugins": {
                "list": {
                    "llama-cpp": {
                        "enable": true,
                        "config": { "mmproj_url": "https://cdn.example/mmproj.gguf" }
                    }
                },
                "enabled": true
            },
            "ai": {
                "local_models": { "gemma-4-e4b": { "url": "https://cdn.example/model.gguf" } },
                "ort_dylib_path": "/opt/onnx/libonnxruntime.so",
                "tasks": { "chat": { "plugin": "echo" } }
            }
        });
        migrate_drop_legacy_plugin_list(&mut doc).expect("drop succeeds");
        assert!(doc.pointer("/plugins/list").is_none());
        assert!(doc.pointer("/plugins/enabled").is_none());
        assert!(doc.pointer("/ai/local_models").is_none());
        assert!(doc.pointer("/ai/ort_dylib_path").is_none());
        assert_eq!(
            doc.pointer("/ai/tasks/chat/plugin"),
            Some(&serde_json::json!("echo"))
        );
    }
}
