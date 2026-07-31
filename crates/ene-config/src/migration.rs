//! Config-version migration for `settings.json`.
//!
//! [`EneConfig`](crate::EneConfig) carries a `version` field, but until this
//! module existed nothing read it. This module is the mechanism that makes the
//! number meaningful: when the on-disk schema changes across application
//! versions, registered migration steps rewrite the *raw JSON* of an old
//! `settings.json` forward, one version at a time, before the figment pipeline
//! deserialises it into the typed [`EneConfig`](crate::EneConfig).
//!
//! # Why raw JSON, not the typed struct
//!
//! Migrations run on [`serde_json::Value`] rather than on
//! [`EneConfig`](crate::EneConfig) because a schema change may alter a field's
//! *type*. If a field that used to be a string becomes an object, deserialising
//! the old file into the current struct fails outright — there is nothing to
//! migrate. Rewriting the JSON first sidesteps that: each step only has to
//! understand the shape of the two versions it bridges.
//!
//! # How steps are registered and run
//!
//! A step is a `fn(&mut serde_json::Value) -> Result<(), EneConfigError>`
//! registered against the version it migrates *from*: a step registered for
//! version `N` rewrites a version-`N` document into a version-`N+1` document.
//! [`apply_migrations`] runs the steps for `from`, `from+1`, … in ascending
//! order until the document reaches [`CURRENT_CONFIG_VERSION`], stamping the
//! `version` field after each step. Steps therefore compose: to migrate from
//! version 1 to 4, the steps for 1, 2, and 3 run in that order.
//!
//! # Version policy
//!
//! * A file **older** than [`CURRENT_CONFIG_VERSION`] is migrated forward and
//!   the new version is persisted by the caller.
//! * A file **at** the current version is returned unchanged.
//! * A file **newer** than the current version (written by a newer build, i.e.
//!   a downgrade) is an error: [`EneConfigError::ConfigVersionTooNew`]. The file
//!   is deliberately left untouched so the newer build can still read it.
//!
//! # Scope
//!
//! This mechanism covers the host `settings.json` only. Per-character
//! `character_settings.json` and character cards (`character.json`) do not
//! carry a `version` field today; applying the same scheme to them is recorded
//! as a follow-up decision (see #330) and is intentionally out of scope here.
//!
//! # Adding a real migration
//!
//! 1. Bump [`CURRENT_CONFIG_VERSION`].
//! 2. Write a step that rewrites a version-`(N-1)` document into version `N`.
//! 3. Register it with [`register_migration`] for `from = N - 1` — typically
//!    from a `ctor` in the crate that owns the affected schema, or eagerly at
//!    startup before the first [`load_config`](crate::load_config).
//!
//! No real schema migrations exist yet; the registry ships empty and the
//! mechanism is exercised by unit tests.

use crate::error::EneConfigError;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The schema version the current build reads and writes.
///
/// A `settings.json` whose `version` is below this is migrated forward on load;
/// one whose `version` exceeds it is rejected (see
/// [`EneConfigError::ConfigVersionTooNew`]). Bump this whenever you register a
/// new migration step.
pub const CURRENT_CONFIG_VERSION: u32 = 1;

/// The JSON key holding the schema version in `settings.json`.
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

/// Writes the `version` field on a raw settings document, creating the root
/// object if the document is `null`.
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

        TEST_VERSION_OVERRIDE.store(version, std::sync::atomic::Ordering::Release);
        registry().lock().clear();
        APPLIED.store(false, std::sync::atomic::Ordering::Release);

        body();

        TEST_VERSION_OVERRIDE.store(0, std::sync::atomic::Ordering::Release);
        registry().lock().clear();
        APPLIED.store(false, std::sync::atomic::Ordering::Release);
    }

    /// A document already at the current version passes through unchanged.
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

    /// A document with no `version` field is treated as version 1 and, when the
    /// current version is 1, returned with the field stamped in.
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

    /// A document newer than the build supports is rejected without mutation.
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

    /// An old document is migrated to the current version on load, and the
    /// `version` field is updated to the current version.
    #[test]
    fn old_version_is_migrated_to_current() {
        with_test_version(2, || {
            // v1 -> v2: rename `name` to `user_name`.
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

    /// When several versions are behind, steps run in ascending order and each
    /// sees the output of the previous one.
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

    /// A gap in the migration chain (no step for some intermediate version) is
    /// a hard error rather than a silent skip.
    #[test]
    fn missing_intermediate_step_errors() {
        with_test_version(3, || {
            // Register 1 -> 2 but not 2 -> 3.
            register_migration(1, |_| Ok(())).expect("registration below current version succeeds");

            let doc = serde_json::json!({ "version": 1 });
            let err = apply_migrations(doc).expect_err("gap in chain must error");
            assert!(
                matches!(err, EneConfigError::GenericConfigError(_)),
                "expected GenericConfigError, got {err:?}"
            );
        });
    }

    /// A non-numeric `version` is reported as an error, not silently defaulted.
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

    /// Registering a step at or above the current version is rejected, since
    /// there is no target version to migrate to, while a valid lower version is
    /// accepted.
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
}
