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
//! *type*. If a field that is a string becomes an object, deserialising
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
//! carry a `version` field today; applying the same scheme to them is a
//! follow-up decision and is intentionally out of scope here.
//!
//! # Adding a real migration
//!
//! 1. Bump [`CURRENT_CONFIG_VERSION`].
//! 2. Write a step that rewrites a version-`(N-1)` document into version `N`.
//! 3. Register it with [`register_migration`] for `from = N - 1` — typically
//!    from a `ctor` in the crate that owns the affected schema, or eagerly at
//!    startup before the first [`load_config`](crate::load_config).
//!
//! One real migration ships today: version 1 → 2 relocates provider-specific
//! settings out of `ai.*` into the `plugins.list.*` sections that now own them
//! (see `migrate_v1_to_v2`). It is registered by a `ctor` in this crate
//! because `ene-config` owns the settings document schema, and the step must be
//! in place wherever a `settings.json` is loaded — the runtime, the desktop
//! app, and the CLI alike.

use crate::error::EneConfigError;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The schema version the current build reads and writes.
///
/// A `settings.json` whose `version` is below this is migrated forward on load;
/// one whose `version` exceeds it is rejected (see
/// [`EneConfigError::ConfigVersionTooNew`]). Bump this whenever you register a
/// new migration step.
pub const CURRENT_CONFIG_VERSION: u32 = 2;

/// The JSON key holding the schema version in `settings.json`.
const VERSION_KEY: &str = "version";

/// Plugin list keys the v1→v2 migration relocates settings into.
///
/// Local literals because `ene-config` must not depend on `ene-ai` (which
/// defines the canonical names in `plugin_config.rs`); the migration tests pin
/// the two to stay in sync.
const LLAMA_CPP_PLUGIN: &str = "llama-cpp";
const ONNX_PLUGIN: &str = "onnx";
const KOKORO_PLUGIN: &str = "kokoro";
/// Default profile name under `plugins.list.kokoro.profiles` for the single
/// Kokoro voice set shipped today.
const KOKORO_DEFAULT_PROFILE: &str = "kokoro";

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

/// A value counts as "present" for the v1→v2 relocation.
///
/// Empty strings and `null` are treated as absent: v1 shipped default-valued
/// entries (`mmproj_url: ""`, `acceleration` always present as `"auto"`), and
/// moving those over an existing plugin config would silently clobber a
/// user-configured value with a default.
fn has_value(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Sets `plugins.list.<plugin>.config.<key>` to `value`, creating the
/// intermediate objects as needed.
fn set_plugin_config_key(
    doc: &mut serde_json::Value,
    plugin: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    let root = doc.as_object_mut().ok_or_else(|| {
        EneConfigError::GenericConfigError("config document is not a JSON object".to_string())
    })?;
    let plugin_entry = object_at(root, "plugins")
        .and_then(|p| object_at(p, "list"))
        .and_then(|l| object_at(l, plugin))
        .ok_or_else(|| {
            EneConfigError::GenericConfigError(
                "plugins.list section is not a JSON object".to_string(),
            )
        })?;
    object_at(plugin_entry, "config")
        .ok_or_else(|| {
            EneConfigError::GenericConfigError(
                "plugins.list.<name>.config is not a JSON object".to_string(),
            )
        })?
        .insert(key.to_string(), value);
    Ok(())
}

/// Sets `plugins.list.<plugin>.profiles.<profile>.<key>` to `value`, creating
/// the intermediate objects as needed.
fn set_plugin_profile_key(
    doc: &mut serde_json::Value,
    plugin: &str,
    profile: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    let root = doc.as_object_mut().ok_or_else(|| {
        EneConfigError::GenericConfigError("config document is not a JSON object".to_string())
    })?;
    let plugin_entry = object_at(root, "plugins")
        .and_then(|p| object_at(p, "list"))
        .and_then(|l| object_at(l, plugin))
        .ok_or_else(|| {
            EneConfigError::GenericConfigError(
                "plugins.list section is not a JSON object".to_string(),
            )
        })?;
    let profiles = object_at(plugin_entry, "profiles").ok_or_else(|| {
        EneConfigError::GenericConfigError(
            "plugins.list.<name>.profiles is not a JSON object".to_string(),
        )
    })?;
    let profile_obj = object_at(profiles, profile).ok_or_else(|| {
        EneConfigError::GenericConfigError(
            "plugins.list.<name>.profiles.<profile> is not a JSON object".to_string(),
        )
    })?;
    profile_obj.insert(key.to_string(), value);
    Ok(())
}

/// Returns the object stored under `key` in `parent`, creating it when absent.
fn object_at<'a>(
    parent: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let value = parent.entry(key).or_insert_with(|| serde_json::json!({}));
    value.as_object_mut()
}

/// Moves the per-model `ai.local_models.<model>.<key>` value into the single
/// plugin-level `plugins.list.llama-cpp.config.<key>` slot.
///
/// The plugin config can hold only one `mmproj`/`acceleration` setting, so the
/// **first model with a non-empty value wins** (in the JSON map's iteration
/// order). The key is then removed from every model: the per-model locations
/// are dead once the value has been collapsed to the plugin level. A no-op
/// when no model carries a value for `key`.
fn move_first_local_model_key(
    doc: &mut serde_json::Value,
    key: &str,
) -> Result<(), EneConfigError> {
    let Some(models) = doc
        .pointer("/ai/local_models")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(());
    };
    let Some(value) = models
        .values()
        .find_map(|model| model.get(key).filter(|v| has_value(v)))
        .cloned()
    else {
        return Ok(());
    };

    if let Some(models) = doc
        .pointer_mut("/ai/local_models")
        .and_then(serde_json::Value::as_object_mut)
    {
        for model in models.values_mut() {
            if let Some(obj) = model.as_object_mut() {
                obj.remove(key);
            }
        }
    }

    set_plugin_config_key(doc, LLAMA_CPP_PLUGIN, key, value)
}

/// Moves `ai.ort_dylib_path` into `plugins.list.onnx.config.ort_dylib_path`.
fn move_ort_dylib_path(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    let Some(value) = doc
        .pointer("/ai/ort_dylib_path")
        .filter(|v| has_value(v))
        .cloned()
    else {
        return Ok(());
    };
    if let Some(ai) = doc
        .pointer_mut("/ai")
        .and_then(serde_json::Value::as_object_mut)
    {
        ai.remove("ort_dylib_path");
    }
    set_plugin_config_key(doc, ONNX_PLUGIN, "ort_dylib_path", value)
}

/// Moves `ai.tts.voices_path` into
/// `plugins.list.kokoro.profiles.kokoro.voices_path`.
fn move_voices_path(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    let Some(value) = doc
        .pointer("/ai/tts/voices_path")
        .filter(|v| has_value(v))
        .cloned()
    else {
        return Ok(());
    };
    if let Some(tts) = doc
        .pointer_mut("/ai/tts")
        .and_then(serde_json::Value::as_object_mut)
    {
        tts.remove("voices_path");
    }
    set_plugin_profile_key(
        doc,
        KOKORO_PLUGIN,
        KOKORO_DEFAULT_PROFILE,
        "voices_path",
        value,
    )
}

/// v1 → v2: relocates provider-specific settings out of `ai.*` into the
/// `plugins.list.*` sections that now own them.
///
/// - `ai.local_models.<model>.mmproj_url` / `mmproj_path` / `acceleration` →
///   `plugins.list.llama-cpp.config.*` (first non-empty model value wins;
///   per-model keys are then dropped)
/// - `ai.ort_dylib_path` → `plugins.list.onnx.config.ort_dylib_path`
/// - `ai.tts.voices_path` → `plugins.list.kokoro.profiles.kokoro.voices_path`
///
/// A no-op (still `Ok`) when there is nothing to move: absent `ai`/`plugins`
/// sections, or only empty-string/`null` values. Existing
/// `plugins.list.<name>.config` values are left untouched unless the old
/// document actually carries a corresponding value.
pub(crate) fn migrate_v1_to_v2(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    // A document without an `ai` section carries none of the relocated keys;
    // the version stamp is applied by `apply_migrations` regardless.
    if doc.get("ai").is_none() {
        return Ok(());
    }

    for key in ["mmproj_url", "mmproj_path", "acceleration"] {
        move_first_local_model_key(doc, key)?;
    }
    move_ort_dylib_path(doc)?;
    move_voices_path(doc)?;
    Ok(())
}

/// Registers the v1→v2 step at process start, wherever a `settings.json` is
/// loaded. `ene-config` owns this step because the `version` field and the
/// migration machinery live here, and the relocated keys are host document
/// schema rather than the property of any single runtime crate.
const _: () = {
    /// # Safety
    ///
    /// Called by `ctor` before `main`. Only safe registration code
    /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
    #[ene_config::ctor(unsafe, crate_path = ene_config)]
    fn register_v1_to_v2_migration() {
        if let Err(err) = register_migration(1, migrate_v1_to_v2) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v1->v2"
            );
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
        // the process without the production migration steps for the next
        // test (the `ctor`-registered v1→v2 step in particular).
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

    /// A full v1 document with every relocated key becomes a v2 document with
    /// the keys at their new `plugins.list.*` locations and the old keys gone.
    #[test]
    fn v1_to_v2_moves_relocated_settings() {
        with_test_version(2, || {
            let mut doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/gemma-4-e4b.gguf",
                            "mmproj_url": "https://cdn.example/mmproj.gguf",
                            "mmproj_path": "/data/mmproj.gguf",
                            "acceleration": "vulkan"
                        }
                    },
                    "ort_dylib_path": "/opt/onnxruntime/libonnxruntime.so",
                    "tts": {
                        "provider": "kokoro",
                        "voices_path": "/data/voices.bin"
                    }
                }
            });
            migrate_v1_to_v2(&mut doc).expect("v1->v2 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_path"),
                Some(&serde_json::json!("/data/mmproj.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/acceleration"),
                Some(&serde_json::json!("vulkan"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/onnx/config/ort_dylib_path"),
                Some(&serde_json::json!("/opt/onnxruntime/libonnxruntime.so"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/kokoro/profiles/kokoro/voices_path"),
                Some(&serde_json::json!("/data/voices.bin"))
            );
            // The old per-model and ai-level keys are removed.
            assert!(
                doc.pointer("/ai/local_models/gemma-4-e4b/mmproj_url")
                    .is_none(),
                "old per-model mmproj_url must be deleted"
            );
            assert!(
                doc.pointer("/ai/local_models/gemma-4-e4b/mmproj_path")
                    .is_none(),
                "old per-model mmproj_path must be deleted"
            );
            assert!(
                doc.pointer("/ai/local_models/gemma-4-e4b/acceleration")
                    .is_none(),
                "old per-model acceleration must be deleted"
            );
            assert!(
                doc.pointer("/ai/ort_dylib_path").is_none(),
                "old ai.ort_dylib_path must be deleted"
            );
            assert!(
                doc.pointer("/ai/tts/voices_path").is_none(),
                "old ai.tts.voices_path must be deleted"
            );
            // The untouched model fields survive.
            assert_eq!(
                doc.pointer("/ai/local_models/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
        });
    }

    /// The first model (in JSON iteration order) carrying a non-empty value
    /// wins, regardless of which model that happens to be: when only one model
    /// has `mmproj_url`, that value is the one relocated.
    #[test]
    fn v1_to_v2_first_non_empty_model_wins() {
        with_test_version(2, || {
            let mut doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "model-a": {
                            "url": "a.gguf",
                            "mmproj_url": "",
                            "mmproj_path": ""
                        },
                        "model-b": {
                            "url": "b.gguf",
                            "mmproj_url": "https://cdn.example/b-mmproj.gguf",
                            "mmproj_path": "/data/b-mmproj.gguf"
                        },
                        "model-c": {
                            "url": "c.gguf",
                            "mmproj_url": "https://cdn.example/c-mmproj.gguf"
                        }
                    }
                }
            });
            migrate_v1_to_v2(&mut doc).expect("v1->v2 migration succeeds");

            // model-b is the first entry with a non-empty mmproj_url; model-c's
            // later value must not win.
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/b-mmproj.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_path"),
                Some(&serde_json::json!("/data/b-mmproj.gguf"))
            );
            // The key is removed from every model, empty-valued or not.
            for model in ["model-a", "model-b", "model-c"] {
                assert!(
                    doc.pointer(&format!("/ai/local_models/{model}/mmproj_url"))
                        .is_none(),
                    "{model} must no longer carry mmproj_url"
                );
            }
        });
    }

    /// Empty-string and null values are "no value": they are neither moved nor
    /// do they clobber an existing plugin config.
    #[test]
    fn v1_to_v2_empty_values_are_not_moved() {
        with_test_version(2, || {
            let mut doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "mmproj_url": "",
                            "mmproj_path": null,
                            "acceleration": "auto"
                        }
                    },
                    "ort_dylib_path": null,
                    "tts": { "voices_path": "" }
                }
            });
            migrate_v1_to_v2(&mut doc).expect("v1->v2 migration succeeds");

            // Empty/null values are not relocated.
            assert!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_url")
                    .is_none(),
                "empty mmproj_url must not be moved"
            );
            assert!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_path")
                    .is_none(),
                "null mmproj_path must not be moved"
            );
            assert!(
                doc.pointer("/plugins/list/onnx").is_none(),
                "null ort_dylib_path must not be moved"
            );
            assert!(
                doc.pointer("/plugins/list/kokoro").is_none(),
                "empty voices_path must not be moved"
            );
            // A non-empty `"auto"` acceleration IS a real value and moves.
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/acceleration"),
                Some(&serde_json::json!("auto"))
            );
        });
    }

    /// A document without an `ai` section has nothing to migrate and is left
    /// untouched (still `Ok`).
    #[test]
    fn v1_to_v2_without_ai_section_is_noop() {
        with_test_version(2, || {
            let mut doc = serde_json::json!({
                "version": 1,
                "character": "Alicia"
            });
            let before = doc.clone();
            migrate_v1_to_v2(&mut doc).expect("no-ai document migrates ok");
            assert_eq!(doc, before, "document without an ai section is unchanged");
        });
    }

    /// Existing `plugins.list.<name>.config` values are left alone when the
    /// old document carries no corresponding key (config wins over defaults).
    #[test]
    fn v1_to_v2_preserves_existing_plugin_config_when_old_key_absent() {
        with_test_version(2, || {
            let mut doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "mmproj_url": "",
                            "acceleration": ""
                        }
                    }
                },
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "enable": true,
                            "config": {
                                "mmproj_url": "https://cdn.example/user-configured.gguf",
                                "acceleration": "cuda"
                            }
                        }
                    }
                }
            });
            migrate_v1_to_v2(&mut doc).expect("v1->v2 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!(
                    "https://cdn.example/user-configured.gguf"
                )),
                "existing plugin config must win over an empty old value"
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/acceleration"),
                Some(&serde_json::json!("cuda"))
            );
        });
    }

    /// The end-to-end chain: a v1 document with the old keys reaches version 2
    /// via [`apply_migrations`] and the relocated values are in place.
    #[test]
    fn apply_migrations_migrates_v1_to_v2() {
        with_test_version(2, || {
            register_migration(1, migrate_v1_to_v2).expect("registration succeeds");

            let doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "mmproj_url": "https://cdn.example/mmproj.gguf"
                        }
                    }
                }
            });
            let migrated = apply_migrations(doc).expect("migration chain succeeds");

            assert_eq!(migrated.get("version"), Some(&serde_json::json!(2)));
            assert_eq!(
                migrated.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
        });
    }
}
