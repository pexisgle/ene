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
//! Four real migrations ship today:
//!
//! - version 1 → 2 relocates provider-specific settings out of `ai.*` into
//!   the `plugins.list.*` sections that now own them (see
//!   `migrate_v1_to_v2`);
//! - version 2 → 3 mirrors each `ai.local_models.<name>` entry's model
//!   path/settings into `plugins.list.llama-cpp.profiles.<name>`, the model
//!   profiles the local GGUF provider plugin consumes (see
//!   `migrate_v2_to_v3`);
//! - version 3 → 4 re-runs that mirror so installs that reached v3 before
//!   `context_size` / `dimensions` became profile keys receive them too (see
//!   `migrate_v3_to_v4`);
//! - version 4 → 5 relocates the voice engine paths and VAD tuning that the
//!   provider plugins now own out of `ai.*` (see `migrate_v4_to_v5`);
//! - version 5 → 6 mirrors the llama-cpp plugin's config and profiles into
//!   the experimental llama-server plugin (see `migrate_v5_to_v6`).
//! - version 6 → 7 reduces `ai.tts` / `ai.stt` to provider routing and moves
//!   every provider-owned value into `plugins.list.<plugin>.config`, plus
//!   renames the VOICEVOX managed-mode keys (see `migrate_v6_to_v7`).
//!
//! They are registered by a `ctor` in this crate because `ene-config` owns
//! the settings document schema, and the steps must be in place wherever a
//! `settings.json` is loaded — the runtime, the desktop app, and the CLI
//! alike.

use crate::error::EneConfigError;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The schema version the current build reads and writes.
///
/// A `settings.json` whose `version` is below this is migrated forward on load;
/// one whose `version` exceeds it is rejected (see
/// [`EneConfigError::ConfigVersionTooNew`]). Bump this whenever you register a
/// new migration step.
pub const CURRENT_CONFIG_VERSION: u32 = 7;

/// The JSON key holding the schema version in `settings.json`.
const VERSION_KEY: &str = "version";

/// Plugin list keys the v1→v2 migration relocates settings into.
///
/// Local literals because `ene-config` must not depend on `ene-ai` (which
/// defines the canonical names in `plugin_config.rs`); the migration tests pin
/// the two to stay in sync.
const LLAMA_CPP_PLUGIN: &str = "llama-cpp";
/// Plugin list key the v5→v6 migration mirrors the llama-cpp settings into.
const LLAMA_SERVER_PLUGIN: &str = "llama-server";
const ONNX_PLUGIN: &str = "onnx";
/// Plugin list key the v4→v5 migration relocates `ai.stt.model_path` into.
const WHISPER_PLUGIN: &str = "whisper";
/// Plugin list key for the `OpenAI` Speech API TTS plugin.
const OPENAI_TTS_PLUGIN: &str = "openai-tts";
/// Plugin list key for the `ElevenLabs` TTS plugin.
const ELEVENLABS_PLUGIN: &str = "elevenlabs";
/// Plugin list key for the VOICEVOX / Aivis Speech TTS plugin.
const VOICEVOX_PLUGIN: &str = "voicevox";
const KOKORO_PLUGIN: &str = "kokoro";
/// Default profile name under `plugins.list.kokoro.profiles` for the single
/// Kokoro voice set shipped today.
const KOKORO_DEFAULT_PROFILE: &str = "kokoro";

/// `ai.local_models.<name>` keys mirrored into
/// `plugins.list.llama-cpp.profiles.<name>` by the v2→v3 migration.
///
/// `context_size` and `dimensions` are mirrored too: the plugin sizes the
/// chat KV cache from the profile's `context_size` (the host's routing window
/// stays in `ai.local_models`), and `dimensions` documents the embedding
/// dimensionality the host needs for the store schema. Both remain in
/// `ai.local_models` as the routing/config source of truth.
const LOCAL_MODEL_PROFILE_KEYS: [&str; 6] = [
    "url",
    "quantization",
    "model_path",
    "gpu_layers",
    "context_size",
    "dimensions",
];

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

/// Moves one `ai.<section>.<key>` value into
/// `plugins.list.<plugin>.config.<key>`, removing it from `ai` afterwards.
///
/// A no-op when the old key is absent, `null`, or an empty string (the same
/// "present" convention as the v1→v2 relocation).
fn move_ai_key_to_plugin_config(
    doc: &mut serde_json::Value,
    from: &str,
    plugin: &str,
    key: &str,
) -> Result<(), EneConfigError> {
    let Some(value) = doc.pointer(from).filter(|v| has_value(v)).cloned() else {
        return Ok(());
    };
    let parent_path = from.rsplit_once('/').map_or(from, |(parent, _)| parent);
    if let Some(parent) = doc
        .pointer_mut(parent_path)
        .and_then(serde_json::Value::as_object_mut)
    {
        parent.remove(key);
    }
    set_plugin_config_key(doc, plugin, key, value)
}

/// v4 → v5: relocates the remaining voice engine settings out of `ai.*`
/// into the provider plugins that now own them.
///
/// - `ai.stt.model_path` → `plugins.list.whisper.config.model_path`
/// - `ai.vad.model` / `model_path` / `threshold` →
///   `plugins.list.onnx.config.{model,model_path,threshold}`
///
/// A no-op (still `Ok`) when there is nothing to move. Existing
/// `plugins.list.*.config` values are left untouched unless the old document
/// actually carries a corresponding value.
pub(crate) fn migrate_v4_to_v5(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    if doc.get("ai").is_none() {
        return Ok(());
    }
    move_ai_key_to_plugin_config(doc, "/ai/stt/model_path", WHISPER_PLUGIN, "model_path")?;
    move_ai_key_to_plugin_config(doc, "/ai/vad/model", ONNX_PLUGIN, "model")?;
    move_ai_key_to_plugin_config(doc, "/ai/vad/model_path", ONNX_PLUGIN, "model_path")?;
    move_ai_key_to_plugin_config(doc, "/ai/vad/threshold", ONNX_PLUGIN, "threshold")?;
    Ok(())
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

/// Returns the existing `plugins.list.<plugin>.profiles.<model>.<key>`
/// value, or `None` when any level of the path is absent.
fn existing_plugin_profile_value<'a>(
    doc: &'a serde_json::Value,
    plugin: &str,
    model: &str,
    key: &str,
) -> Option<&'a serde_json::Value> {
    doc.get("plugins")?
        .get("list")?
        .get(plugin)?
        .get("profiles")?
        .get(model)?
        .get(key)
}

/// Mirrors one `ai.local_models.<model>` field into
/// `plugins.list.<plugin>.profiles.<model>`, unless the profile already
/// carries a non-empty value for that key (explicit plugin config wins over
/// the mirror).
fn mirror_local_model_profile_key_for_plugin(
    doc: &mut serde_json::Value,
    plugin: &str,
    model: &str,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), EneConfigError> {
    if !has_value(value)
        || existing_plugin_profile_value(doc, plugin, model, key).is_some_and(has_value)
    {
        return Ok(());
    }
    set_plugin_profile_key(doc, plugin, model, key, value.clone())
}

/// Mirrors the model path/settings of every `ai.local_models` entry into the
/// `<plugin>`'s `profiles.<name>` blob.
///
/// This is a one-way mirror, not a move: `ai.local_models` stays intact
/// because `ene-ai` still routes tasks and budgets context windows from it.
/// Empty-string / `null` values are not mirrored, and a non-empty existing
/// profile value is never overwritten (an existing empty-string / `null`
/// value counts as absent, matching the v1→v2 convention). A no-op (still
/// `Ok`) when there are no local models.
fn mirror_local_models_into_plugin_profiles(
    doc: &mut serde_json::Value,
    plugin: &str,
) -> Result<(), EneConfigError> {
    let Some(models) = doc
        .pointer("/ai/local_models")
        .and_then(serde_json::Value::as_object)
        .cloned()
    else {
        return Ok(());
    };
    for (model, entry) in &models {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        for key in LOCAL_MODEL_PROFILE_KEYS {
            if let Some(value) = entry.get(key) {
                mirror_local_model_profile_key_for_plugin(doc, plugin, model, key, value)?;
            }
        }
    }
    Ok(())
}

/// v2 → v3: mirrors `ai.local_models` into the llama-cpp plugin profiles.
pub(crate) fn migrate_v2_to_v3(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    mirror_local_models_into_plugin_profiles(doc, LLAMA_CPP_PLUGIN)
}

/// v3 → v4: re-runs the `ai.local_models` → llama-cpp profile mirror.
///
/// The v2→v3 step only ran for files stamped v2, so installs that migrated
/// before `context_size` / `dimensions` joined
/// [`LOCAL_MODEL_PROFILE_KEYS`](LOCAL_MODEL_PROFILE_KEYS) still have profiles
/// without them; without this step their local embedding setup fails at
/// startup with a "requires `ai.local_models`.<name>.dimensions" error until
/// a hand edit. Same fill-only-missing-keys semantics as the v2→v3 step, so
/// a v4 file re-run is a no-op.
pub(crate) fn migrate_v3_to_v4(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    mirror_local_models_into_plugin_profiles(doc, LLAMA_CPP_PLUGIN)
}

/// Writes `plugins.list.<plugin>.config.<key>` only when the slot is absent
/// or empty (existing explicit values win over the mirror).
fn fill_plugin_config_key(
    doc: &mut serde_json::Value,
    plugin: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    let path = format!("/plugins/list/{plugin}/config/{key}");
    if doc.pointer(&path).is_some_and(has_value) {
        return Ok(());
    }
    set_plugin_config_key(doc, plugin, key, value)
}

/// Writes `plugins.list.<plugin>.profiles.<model>.<key>` only when the slot
/// is absent or empty.
fn fill_plugin_profile_key(
    doc: &mut serde_json::Value,
    plugin: &str,
    model: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<(), EneConfigError> {
    if existing_plugin_profile_value(doc, plugin, model, key).is_some_and(has_value) {
        return Ok(());
    }
    set_plugin_profile_key(doc, plugin, model, key, value)
}

/// v5 → v6: mirrors the llama-cpp plugin's config and profiles into the
/// experimental llama-server plugin, and re-runs the `ai.local_models`
/// profile mirror for it.
///
/// The llama-server plugin is the sidecar-based successor to the in-process
/// llama-cpp plugin and starts disabled, so existing users keep working
/// without hand-editing: every non-empty llama-cpp config key and profile
/// value is copied over, and `ai.local_models` fills any profile keys the
/// llama-cpp section never carried. Same fill-only-missing semantics as the
/// older mirrors: an existing non-empty llama-server value is never
/// overwritten. The llama-cpp section is left untouched so the old plugin
/// keeps working until the switch-over is complete.
pub(crate) fn migrate_v5_to_v6(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    let llama_cpp_config = doc
        .pointer("/plugins/list/llama-cpp/config")
        .and_then(serde_json::Value::as_object)
        .cloned();
    if let Some(config) = llama_cpp_config {
        for (key, value) in &config {
            if has_value(value) {
                fill_plugin_config_key(doc, LLAMA_SERVER_PLUGIN, key, value.clone())?;
            }
        }
    }
    let llama_cpp_profiles = doc
        .pointer("/plugins/list/llama-cpp/profiles")
        .and_then(serde_json::Value::as_object)
        .cloned();
    if let Some(profiles) = llama_cpp_profiles {
        for (model, raw) in &profiles {
            let Some(entry) = raw.as_object() else {
                continue;
            };
            for (key, value) in entry {
                if has_value(value) {
                    fill_plugin_profile_key(doc, LLAMA_SERVER_PLUGIN, model, key, value.clone())?;
                }
            }
        }
    }
    mirror_local_models_into_plugin_profiles(doc, LLAMA_SERVER_PLUGIN)
}

/// Returns the `plugins.list.<plugin>.config` object, creating the
/// intermediate `plugins.list.<plugin>` entry when the plugin section is
/// absent. `None` when `plugins.list.<plugin>` exists but is not an object.
fn plugin_config_object<'a>(
    doc: &'a mut serde_json::Value,
    plugin: &str,
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let root = doc.as_object_mut()?;
    let list = object_at(root, "plugins")?;
    let plugin_entry = object_at(list, "list")?;
    let entry = object_at(plugin_entry, plugin)?;
    // A `null` config blob (the v6 default for most built-in plugins) is
    // normalized to an empty object: the v7 schema treats null as "no
    // provider-owned values yet", and the relocation must not fail on the
    // stock configuration. Non-object scalars/arrays keep failing loudly
    // rather than silently discarding a user value.
    let config = entry
        .entry("config")
        .or_insert_with(|| serde_json::json!({}));
    if config.is_null() {
        *config = serde_json::json!({});
    }
    config.as_object_mut()
}

/// Destination plugin + key for one legacy `ai.tts` / `ai.stt` value.
///
/// Well-known providers map onto their plugin's config keys (some renamed,
/// e.g. VOICEVOX `voice` → `speaker_id`); unknown providers get the same
/// key inside `plugins.list.<provider>.config` so a future plugin can pick
/// the values up without a second migration.
fn tts_destination<'a>(provider: &'a str, key: &'a str) -> (&'a str, &'a str) {
    match (provider, key) {
        ("kokoro", _) => (KOKORO_PLUGIN, key),
        ("openai_tts", _) => (OPENAI_TTS_PLUGIN, key),
        ("elevenlabs", "model") => (ELEVENLABS_PLUGIN, "model_id"),
        ("elevenlabs", "voice") => (ELEVENLABS_PLUGIN, "voice_id"),
        ("elevenlabs", _) => (ELEVENLABS_PLUGIN, key),
        ("voicevox", "voice") => (VOICEVOX_PLUGIN, "speaker_id"),
        ("voicevox", "speed") => (VOICEVOX_PLUGIN, "speed_scale"),
        ("voicevox", _) => (VOICEVOX_PLUGIN, key),
        (other, _) => (other, key),
    }
}

/// Moves every non-routing key out of `ai.<section>` into the selected
/// provider plugin's config, then deletes the dead keys from `ai.<section>`.
///
/// Existing non-empty destination values win (fill-only-missing, matching
/// the earlier relocation steps). Keys with no value (empty strings, `null`)
/// are removed without being written anywhere.
fn relocate_ai_section_to_provider_plugin(
    doc: &mut serde_json::Value,
    section: &str,
) -> Result<(), EneConfigError> {
    let Some(ai) = doc.pointer(&format!("/ai/{section}")) else {
        return Ok(());
    };
    let provider = ai
        .get("provider")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "none")
        .map(str::to_string);
    let entries: Vec<(String, serde_json::Value)> = ai
        .as_object()
        .into_iter()
        .flat_map(serde_json::Map::iter)
        .filter(|(key, _)| key.as_str() != "provider")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let _ = ai;
    if let Some(ai) = doc
        .pointer_mut(&format!("/ai/{section}"))
        .and_then(serde_json::Value::as_object_mut)
    {
        for (key, _) in &entries {
            ai.remove(key);
        }
    }
    for (key, value) in entries {
        if !has_value(&value) {
            continue;
        }
        let Some(provider) = provider.as_deref() else {
            continue;
        };
        let (plugin, destination) = tts_destination(provider, &key);
        let Some(config) = plugin_config_object(doc, plugin) else {
            return Err(EneConfigError::GenericConfigError(format!(
                "plugins.list.{plugin} is not a JSON object; cannot relocate ai.{section}.{key}"
            )));
        };
        let value = if plugin == VOICEVOX_PLUGIN && destination == "speaker_id" {
            match value {
                serde_json::Value::String(text) => text.trim().parse::<u64>().map_or_else(
                    |_| {
                        tracing::warn!(
                            component = "Config",
                            value = %text,
                            "ai.tts.voice is not a numeric VOICEVOX speaker id; \
                             leaving speaker_id untouched"
                        );
                        serde_json::Value::Null
                    },
                    serde_json::Value::from,
                ),
                other => other,
            }
        } else {
            value
        };
        if value.is_null() {
            continue;
        }
        if config
            .get(destination)
            .is_none_or(|existing| !has_value(existing))
        {
            config.insert(destination.to_string(), value);
        }
    }
    Ok(())
}

/// Renames the legacy VOICEVOX managed-mode keys onto the unified sidecar
/// naming, then removes the old keys.
///
/// `auto_start` (bool) becomes `mode` (`"managed"` / `"external"`);
/// `engine_path` / `engine_args` become `server_path` / `server_args`.
/// Existing non-empty `mode` / `server_path` / `server_args` values win; the
/// old keys are always removed because the v7 plugin no longer reads them.
fn migrate_voicevox_sidecar_keys(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    // Only touch documents that actually configure the plugin; an absent
    // voicevox entry must not be synthesized by the migration.
    if doc
        .pointer(&format!("/plugins/list/{VOICEVOX_PLUGIN}"))
        .is_none()
    {
        return Ok(());
    }
    let Some(config) = plugin_config_object(doc, VOICEVOX_PLUGIN) else {
        return Err(EneConfigError::GenericConfigError(
            "plugins.list.voicevox.config is not a JSON object; \
             cannot migrate managed-mode keys"
                .to_string(),
        ));
    };
    let mode_missing = config
        .get("mode")
        .is_none_or(|existing| !has_value(existing));
    if mode_missing {
        match config.remove("auto_start") {
            Some(serde_json::Value::Bool(true)) => {
                config.insert("mode".to_string(), serde_json::json!("managed"));
            }
            Some(_) | None => {
                config.insert("mode".to_string(), serde_json::json!("external"));
            }
        }
    } else {
        config.remove("auto_start");
    }
    if let Some(path) = config.remove("engine_path")
        && has_value(&path)
        && config
            .get("server_path")
            .is_none_or(|existing| !has_value(existing))
    {
        config.insert("server_path".to_string(), path);
    }
    if let Some(args) = config.remove("engine_args")
        && has_value(&args)
        && config
            .get("server_args")
            .is_none_or(|existing| !has_value(existing))
    {
        config.insert("server_args".to_string(), args);
    }
    Ok(())
}

/// v6 → v7: `ai.tts` / `ai.stt` become provider routing only.
///
/// Every provider-owned value moves into the selected provider plugin's
/// `plugins.list.<plugin>.config` (the single source the host adapters and
/// the settings UI use from v7 on), and the VOICEVOX config switches to the
/// unified `mode` / `server_path` / `server_args` sidecar naming. Unknown
/// providers receive the same-named keys so future plugins can consume them;
/// existing non-empty destination values are never overwritten; re-running
/// the step on a v7 document is a no-op.
pub(crate) fn migrate_v6_to_v7(doc: &mut serde_json::Value) -> Result<(), EneConfigError> {
    relocate_ai_section_to_provider_plugin(doc, "tts")?;
    relocate_ai_section_to_provider_plugin(doc, "stt")?;
    migrate_voicevox_sidecar_keys(doc)
}

/// Registers the v1→v2 … v6→v7 steps at process start, wherever a
/// `settings.json` is loaded. `ene-config` owns these steps because the
/// `version` field and the migration machinery live here, and the relocated
/// keys are host document schema rather than the property of any single
/// runtime crate.
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
        if let Err(err) = register_migration(2, migrate_v2_to_v3) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v2->v3"
            );
        }
        if let Err(err) = register_migration(3, migrate_v3_to_v4) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v3->v4"
            );
        }
        if let Err(err) = register_migration(4, migrate_v4_to_v5) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v4->v5"
            );
        }
        if let Err(err) = register_migration(5, migrate_v5_to_v6) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v5->v6"
            );
        }
        if let Err(err) = register_migration(6, migrate_v6_to_v7) {
            tracing::error!(
                component = "Config",
                error = %err,
                "failed to register settings.json migration v6->v7"
            );
        }
    }
};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    type FieldMapping = (&'static str, &'static str);
    type VoiceProviderMigrationCase = (&'static str, &'static str, &'static [FieldMapping]);

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

    /// The v4→v5 step relocates the voice engine settings into the plugins
    /// that now own them, and leaves the selection fields behind.
    #[test]
    fn v4_to_v5_moves_voice_engine_settings() {
        with_test_version(5, || {
            let mut doc = serde_json::json!({
                "version": 4,
                "ai": {
                    "stt": {
                        "provider": "whisper",
                        "model": "small.gguf",
                        "language": "ja",
                        "model_path": "/data/whisper.gguf"
                    },
                    "vad": {
                        "provider": "silero",
                        "model": "silero_vad.onnx",
                        "model_path": "/data/silero.onnx",
                        "threshold": 0.7
                    }
                }
            });
            migrate_v4_to_v5(&mut doc).expect("v4->v5 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/whisper/config/model_path"),
                Some(&serde_json::json!("/data/whisper.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/onnx/config/model"),
                Some(&serde_json::json!("silero_vad.onnx"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/onnx/config/model_path"),
                Some(&serde_json::json!("/data/silero.onnx"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/onnx/config/threshold"),
                Some(&serde_json::json!(0.7))
            );
            assert!(doc.pointer("/ai/stt/model_path").is_none());
            assert!(doc.pointer("/ai/vad/model").is_none());
            assert!(doc.pointer("/ai/vad/model_path").is_none());
            assert!(doc.pointer("/ai/vad/threshold").is_none());
            // Selection fields stay in `ai.*`.
            assert_eq!(
                doc.pointer("/ai/stt/provider"),
                Some(&serde_json::json!("whisper"))
            );
            assert_eq!(
                doc.pointer("/ai/vad/provider"),
                Some(&serde_json::json!("silero"))
            );
        });
    }

    /// Empty-string / `null` old values and absent sections are left alone.
    #[test]
    fn v4_to_v5_empty_values_are_not_moved() {
        with_test_version(5, || {
            let mut doc = serde_json::json!({
                "version": 4,
                "ai": {
                    "stt": { "model_path": "" },
                    "vad": { "threshold": null }
                }
            });
            migrate_v4_to_v5(&mut doc).expect("v4->v5 migration succeeds");
            assert!(doc.pointer("/plugins/list/whisper").is_none());
            assert!(doc.pointer("/plugins/list/onnx").is_none());
        });
    }

    /// The v5→v6 step mirrors the llama-cpp config and profiles into
    /// llama-server and re-runs the `ai.local_models` mirror, leaving the
    /// llama-cpp section untouched.
    #[test]
    fn v5_to_v6_mirrors_llama_cpp_into_llama_server() {
        with_test_version(6, || {
            let mut doc = serde_json::json!({
                "version": 5,
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "config": {
                                "mmproj_url": "https://cdn.example/mmproj.gguf",
                                "acceleration": "vulkan"
                            },
                            "profiles": {
                                "gemma-4-e4b": {
                                    "url": "https://cdn.example/gemma-4-e4b.gguf",
                                    "context_size": 4096
                                }
                            }
                        }
                    }
                },
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/gemma-4-e4b.gguf",
                            "dimensions": 1024
                        }
                    }
                }
            });
            migrate_v5_to_v6(&mut doc).expect("v5->v6 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-server/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/config/acceleration"),
                Some(&serde_json::json!("vulkan"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/profiles/gemma-4-e4b/context_size"),
                Some(&serde_json::json!(4096))
            );
            // The ai.local_models mirror fills keys the llama-cpp profile did
            // not carry.
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/profiles/gemma-4-e4b/dimensions"),
                Some(&serde_json::json!(1024))
            );
            // The source section is untouched.
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
            assert!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/dimensions")
                    .is_none()
            );
        });
    }

    /// Existing non-empty llama-server values win; empty slots are still
    /// filled. A document with no llama-cpp section is a no-op.
    #[test]
    fn v5_to_v6_fills_only_missing_keys() {
        with_test_version(6, || {
            let mut doc = serde_json::json!({
                "version": 5,
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "config": {
                                "mmproj_url": "https://cdn.example/mmproj.gguf",
                                "acceleration": "cuda"
                            },
                            "profiles": {
                                "gemma-4-e4b": {
                                    "url": "https://cdn.example/gemma-4-e4b.gguf",
                                    "model_path": "/data/model.gguf"
                                }
                            }
                        },
                        "llama-server": {
                            "config": {
                                "mmproj_url": "https://cdn.example/custom-mmproj.gguf"
                            },
                            "profiles": {
                                "gemma-4-e4b": {
                                    "url": "https://cdn.example/custom.gguf"
                                }
                            }
                        }
                    }
                }
            });
            migrate_v5_to_v6(&mut doc).expect("v5->v6 migration succeeds");

            // Existing non-empty values win.
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/custom-mmproj.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/custom.gguf"))
            );
            // Empty / missing slots are filled.
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/config/acceleration"),
                Some(&serde_json::json!("cuda"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-server/profiles/gemma-4-e4b/model_path"),
                Some(&serde_json::json!("/data/model.gguf"))
            );
        });

        with_test_version(6, || {
            let mut doc = serde_json::json!({ "version": 5, "ai": {} });
            migrate_v5_to_v6(&mut doc).expect("empty v5->v6 migration succeeds");
            assert!(doc.pointer("/plugins/list").is_none());
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

    /// A v2 document's `ai.local_models` entries are mirrored into
    /// `plugins.list.llama-cpp.profiles.<name>` without touching the originals
    /// (routing still reads them). `context_size` and `dimensions` are
    /// mirrored too: the plugin sizes its chat KV cache from the profile's
    /// `context_size`, and `dimensions` is the host's store-schema value.
    #[test]
    fn v2_to_v3_mirrors_local_models_into_profiles() {
        with_test_version(3, || {
            let mut doc = serde_json::json!({
                "version": 2,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/gemma-4-e4b.gguf",
                            "quantization": "Q4_0",
                            "model_path": "/data/gemma.gguf",
                            "gpu_layers": "33",
                            "context_size": 8192,
                            "dimensions": 1024
                        }
                    }
                }
            });
            migrate_v2_to_v3(&mut doc).expect("v2->v3 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/quantization"),
                Some(&serde_json::json!("Q4_0"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/model_path"),
                Some(&serde_json::json!("/data/gemma.gguf"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/gpu_layers"),
                Some(&serde_json::json!("33"))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/context_size"),
                Some(&serde_json::json!(8192))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/dimensions"),
                Some(&serde_json::json!(1024))
            );
            // The source entry and the routing-only field survive untouched.
            assert_eq!(
                doc.pointer("/ai/local_models/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
            assert_eq!(
                doc.pointer("/ai/local_models/gemma-4-e4b/context_size"),
                Some(&serde_json::json!(8192))
            );
            assert_eq!(
                doc.pointer("/ai/local_models/gemma-4-e4b/dimensions"),
                Some(&serde_json::json!(1024))
            );
        });
    }

    /// A v3 document whose profiles were mirrored before `context_size` /
    /// `dimensions` joined the key set is refilled by the v3→v4 step; the
    /// step is a no-op when the keys are already present.
    #[test]
    fn v3_to_v4_refills_missing_profile_keys() {
        with_test_version(4, || {
            let mut doc = serde_json::json!({
                "version": 3,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/gemma-4-e4b.gguf",
                            "gpu_layers": "33",
                            "context_size": 8192,
                            "dimensions": 1024
                        }
                    }
                },
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "profiles": {
                                // Mirrored by the old v2→v3 step, which only
                                // knew the original four keys.
                                "gemma-4-e4b": {
                                    "url": "https://cdn.example/gemma-4-e4b.gguf",
                                    "gpu_layers": "33"
                                }
                            }
                        }
                    }
                }
            });
            migrate_v3_to_v4(&mut doc).expect("v3->v4 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/context_size"),
                Some(&serde_json::json!(8192))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/dimensions"),
                Some(&serde_json::json!(1024))
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
            // A second run must not change anything (idempotent).
            let before = doc.clone();
            migrate_v3_to_v4(&mut doc).expect("v3->v4 re-run succeeds");
            assert_eq!(doc, before);
        });
    }

    /// An existing non-empty profile value wins over the v3→v4 refill, the
    /// same fill-only-missing-keys semantics as the v2→v3 step.
    #[test]
    fn v3_to_v4_preserves_existing_profile_values() {
        with_test_version(4, || {
            let mut doc = serde_json::json!({
                "version": 3,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "context_size": 8192,
                            "dimensions": 1024
                        }
                    }
                },
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "profiles": {
                                "gemma-4-e4b": {
                                    "context_size": 16384
                                }
                            }
                        }
                    }
                }
            });
            migrate_v3_to_v4(&mut doc).expect("v3->v4 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/context_size"),
                Some(&serde_json::json!(16384)),
                "existing profile context_size must not be overwritten"
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/dimensions"),
                Some(&serde_json::json!(1024)),
                "missing dimensions are still filled"
            );
        });
    }

    /// An existing profile value wins over the mirror: the migration never
    /// overwrites explicitly configured plugin profiles.
    #[test]
    fn v2_to_v3_preserves_existing_profile_values() {
        with_test_version(3, || {
            let mut doc = serde_json::json!({
                "version": 2,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/old.gguf",
                            "gpu_layers": "auto"
                        }
                    }
                },
                "plugins": {
                    "list": {
                        "llama-cpp": {
                            "profiles": {
                                "gemma-4-e4b": {
                                    "url": "https://cdn.example/user-pinned.gguf"
                                }
                            }
                        }
                    }
                }
            });
            migrate_v2_to_v3(&mut doc).expect("v2->v3 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/user-pinned.gguf")),
                "existing profile url must not be overwritten"
            );
            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/gpu_layers"),
                Some(&serde_json::json!("auto")),
                "absent profile keys are still mirrored"
            );
        });
    }

    /// Empty-string / `null` model fields are not mirrored (the v1→v2
    /// convention: defaults are not values).
    #[test]
    fn v2_to_v3_skips_empty_values() {
        with_test_version(3, || {
            let mut doc = serde_json::json!({
                "version": 2,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "",
                            "model_path": null,
                            "gpu_layers": "auto"
                        }
                    }
                }
            });
            migrate_v2_to_v3(&mut doc).expect("v2->v3 migration succeeds");

            assert_eq!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/gpu_layers"),
                Some(&serde_json::json!("auto"))
            );
            assert!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/url")
                    .is_none(),
                "empty url must not be mirrored"
            );
            assert!(
                doc.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/model_path")
                    .is_none(),
                "null model_path must not be mirrored"
            );
        });
    }

    /// A document without local models is left untouched.
    #[test]
    fn v2_to_v3_without_local_models_is_noop() {
        with_test_version(3, || {
            let mut doc = serde_json::json!({
                "version": 2,
                "plugins": {
                    "list": {
                        "llama-cpp": { "enable": true }
                    }
                }
            });
            let before = doc.clone();
            migrate_v2_to_v3(&mut doc).expect("no-local-models document migrates ok");
            assert_eq!(doc, before);
        });
    }

    /// The full chain: a v1 document reaches version 3 with both the
    /// v1→v2 relocations and the v2→v3 profile mirror in place.
    #[test]
    fn apply_migrations_migrates_v1_to_v3() {
        with_test_version(3, || {
            register_migration(1, migrate_v1_to_v2).expect("registration succeeds");
            register_migration(2, migrate_v2_to_v3).expect("registration succeeds");

            let doc = serde_json::json!({
                "version": 1,
                "ai": {
                    "local_models": {
                        "gemma-4-e4b": {
                            "url": "https://cdn.example/gemma-4-e4b.gguf",
                            "mmproj_url": "https://cdn.example/mmproj.gguf",
                            "acceleration": "vulkan",
                            "gpu_layers": "33"
                        }
                    }
                }
            });
            let migrated = apply_migrations(doc).expect("migration chain succeeds");

            assert_eq!(migrated.get("version"), Some(&serde_json::json!(3)));
            assert_eq!(
                migrated.pointer("/plugins/list/llama-cpp/config/mmproj_url"),
                Some(&serde_json::json!("https://cdn.example/mmproj.gguf"))
            );
            assert_eq!(
                migrated.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf"))
            );
            assert_eq!(
                migrated.pointer("/plugins/list/llama-cpp/profiles/gemma-4-e4b/gpu_layers"),
                Some(&serde_json::json!("33"))
            );
            assert!(
                migrated
                    .pointer("/ai/local_models/gemma-4-e4b/mmproj_url")
                    .is_none(),
                "v1->v2 still removes the relocated mmproj key"
            );
            assert_eq!(
                migrated.pointer("/ai/local_models/gemma-4-e4b/url"),
                Some(&serde_json::json!("https://cdn.example/gemma-4-e4b.gguf")),
                "the mirror keeps the routing entry intact"
            );
        });
    }

    /// The v6→v7 step moves every known TTS provider's values into the
    /// provider plugin config and reduces `ai.tts` to routing.
    #[test]
    fn v6_to_v7_relocates_known_tts_provider_values() {
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": {
                "tts": {
                    "provider": "voicevox",
                    "model": "legacy-model",
                    "voice": "42",
                    "speed": 1.25,
                    "language": "ja",
                    "model_path": "/data/tts.onnx",
                    "future_key": "preserved"
                },
                "stt": { "provider": "none", "model": "dead" }
            },
            "plugins": {
                "list": {
                    "voicevox": {
                        "config": {
                            "speaker_id": 7,
                            "auto_start": true,
                            "engine_path": "/opt/voicevox/run",
                            "engine_args": ["--port", "50021"]
                        }
                    }
                }
            }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");

        let tts = doc.pointer("/ai/tts").expect("ai.tts remains");
        assert_eq!(tts.get("provider"), Some(&serde_json::json!("voicevox")));
        assert_eq!(tts.as_object().expect("object").len(), 1, "routing only");
        let config = doc
            .pointer("/plugins/list/voicevox/config")
            .expect("voicevox config");
        assert_eq!(
            config.get("speaker_id"),
            Some(&serde_json::json!(7)),
            "existing wins"
        );
        assert_eq!(config.get("mode"), Some(&serde_json::json!("managed")));
        assert_eq!(
            config.get("server_path"),
            Some(&serde_json::json!("/opt/voicevox/run"))
        );
        assert_eq!(
            config.get("server_args"),
            Some(&serde_json::json!(["--port", "50021"]))
        );
        assert!(config.get("auto_start").is_none());
        assert!(config.get("engine_path").is_none());
        assert!(config.get("engine_args").is_none());
        assert_eq!(
            config.get("future_key"),
            Some(&serde_json::json!("preserved")),
            "unknown keys are preserved in the provider config"
        );
        let stt = doc.pointer("/ai/stt").expect("ai.stt remains");
        assert_eq!(stt.as_object().expect("object").len(), 1, "routing only");
    }

    /// Unknown providers receive the same-named keys so a future plugin can
    /// consume them without another migration.
    #[test]
    fn v6_to_v7_unknown_provider_gets_same_named_keys() {
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": {
                "tts": {
                    "provider": "future-tts",
                    "model": "m1",
                    "voice": "v1",
                    "custom": {"nested": true}
                }
            }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let config = doc
            .pointer("/plugins/list/future-tts/config")
            .expect("unknown provider config created");
        assert_eq!(config.get("model"), Some(&serde_json::json!("m1")));
        assert_eq!(config.get("voice"), Some(&serde_json::json!("v1")));
        assert_eq!(
            config.get("custom"),
            Some(&serde_json::json!({"nested": true}))
        );
        assert_eq!(
            doc.pointer("/ai/tts")
                .expect("ai.tts")
                .as_object()
                .expect("object")
                .len(),
            1
        );
    }

    /// `Kokoro`, `openai_tts`, and `ElevenLabs` use their plugin-specific key
    /// mappings; an existing destination value always wins.
    #[test]
    fn v6_to_v7_maps_provider_specific_keys_and_preserves_existing() {
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": {
                "tts": {
                    "provider": "elevenlabs",
                    "model": "eleven_v2",
                    "voice": "Rachel",
                    "speed": 1.1
                },
                "stt": {
                    "provider": "whisper",
                    "model": "ggml-small.bin",
                    "language": "en"
                }
            },
            "plugins": {
                "list": {
                    "elevenlabs": { "config": { "voice_id": "already-set" } }
                }
            }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let eleven = doc
            .pointer("/plugins/list/elevenlabs/config")
            .expect("elevenlabs config");
        assert_eq!(
            eleven.get("model_id"),
            Some(&serde_json::json!("eleven_v2"))
        );
        assert_eq!(
            eleven.get("voice_id"),
            Some(&serde_json::json!("already-set")),
            "existing value wins"
        );
        assert_eq!(eleven.get("speed"), Some(&serde_json::json!(1.1)));
        let whisper = doc
            .pointer("/plugins/list/whisper/config")
            .expect("whisper config");
        assert_eq!(
            whisper.get("model"),
            Some(&serde_json::json!("ggml-small.bin"))
        );
        assert_eq!(whisper.get("language"), Some(&serde_json::json!("en")));
        assert_eq!(
            doc.pointer("/ai/stt")
                .expect("ai.stt")
                .as_object()
                .expect("object")
                .len(),
            1
        );
    }

    /// Re-running the v6→v7 step on an already-migrated document is a no-op
    /// (idempotence): values stay put and `mode` is not overwritten.
    #[test]
    fn v6_to_v7_is_idempotent() {
        let mut doc = serde_json::json!({
            "version": 7,
            "ai": { "tts": { "provider": "voicevox" } },
            "plugins": {
                "list": {
                    "voicevox": {
                        "config": {
                            "mode": "external",
                            "speaker_id": 3
                        }
                    }
                }
            }
        });
        let before = doc.clone();
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        assert_eq!(doc, before);
    }

    /// The legacy VOICEVOX `voice` value is a JSON string; `speaker_id` is a
    /// number, so the migration converts numeric strings and safely ignores
    /// non-numeric ones.
    #[test]
    fn v6_to_v7_voicevox_voice_converts_to_numeric_speaker_id() {
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": { "tts": { "provider": "voicevox", "voice": "42" } },
            "plugins": { "list": { "voicevox": { "config": { "speaker_id": 7 } } } }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let config = doc
            .pointer("/plugins/list/voicevox/config")
            .expect("voicevox config");
        assert_eq!(
            config.get("speaker_id"),
            Some(&serde_json::json!(7)),
            "existing numeric speaker_id wins over the legacy string"
        );

        let mut doc = serde_json::json!({
            "version": 6,
            "ai": { "tts": { "provider": "voicevox", "voice": "42" } }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let config = doc
            .pointer("/plugins/list/voicevox/config")
            .expect("voicevox config");
        assert_eq!(
            config.get("speaker_id"),
            Some(&serde_json::json!(42)),
            "numeric string is converted to a number"
        );

        let mut doc = serde_json::json!({
            "version": 6,
            "ai": { "tts": { "provider": "voicevox", "voice": "not-a-speaker" } },
            "plugins": { "list": { "voicevox": { "config": { "speaker_id": 3 } } } }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let config = doc
            .pointer("/plugins/list/voicevox/config")
            .expect("voicevox config");
        assert_eq!(
            config.get("speaker_id"),
            Some(&serde_json::json!(3)),
            "non-numeric legacy voice leaves the existing speaker untouched"
        );
    }

    /// Every built-in TTS provider and the built-in STT provider relocate
    /// their legacy `ai.*` values onto the provider-owned config keys.
    #[test]
    fn v6_to_v7_covers_every_builtin_voice_provider() {
        // (provider kind, plugin key, expected destination mapping)
        let cases: &[VoiceProviderMigrationCase] = &[
            (
                "kokoro",
                "kokoro",
                &[("model", "model"), ("voice", "voice"), ("speed", "speed")],
            ),
            (
                "openai_tts",
                "openai-tts",
                &[("model", "model"), ("voice", "voice"), ("speed", "speed")],
            ),
            (
                "edge-tts",
                "edge-tts",
                &[("voice", "voice"), ("language", "language")],
            ),
            (
                "elevenlabs",
                "elevenlabs",
                &[("model", "model_id"), ("voice", "voice_id")],
            ),
            (
                "voicevox",
                "voicevox",
                &[("voice", "speaker_id"), ("speed", "speed_scale")],
            ),
        ];
        for (provider, plugin, mappings) in cases {
            let mut doc = serde_json::json!({
                "version": 6,
                "ai": {
                    "tts": {
                        "provider": provider,
                        "model": "m1",
                        "voice": if *provider == "voicevox" { serde_json::json!("2") } else { serde_json::json!("v1") },
                        "speed": 1.1,
                        "language": "ja",
                        "unknown_key": "kept"
                    }
                }
            });
            let root = doc.as_object_mut().expect("root");
            let plugins = root
                .entry("plugins")
                .or_insert_with(|| serde_json::json!({ "list": {} }));
            let list = plugins["list"]
                .as_object_mut()
                .expect("plugins.list object");
            list.insert(
                (*plugin).to_string(),
                serde_json::json!({ "enable": true, "config": null }),
            );
            migrate_v6_to_v7(&mut doc).expect("migration succeeds");
            let config = doc
                .pointer(&format!("/plugins/list/{plugin}/config"))
                .expect("provider config created");
            for (source, destination) in *mappings {
                let expected = if *source == "voice" && *provider == "voicevox" {
                    serde_json::json!(2)
                } else if *source == "voice" {
                    serde_json::json!("v1")
                } else if *source == "model" {
                    serde_json::json!("m1")
                } else if *source == "speed" {
                    serde_json::json!(1.1)
                } else {
                    serde_json::json!("ja")
                };
                assert_eq!(
                    config.get(*destination),
                    Some(&expected),
                    "{provider}: {source} → {destination}"
                );
            }
            assert_eq!(
                config.get("unknown_key"),
                Some(&serde_json::json!("kept")),
                "{provider}: unknown keys are preserved"
            );
            assert_eq!(
                doc.pointer("/ai/tts")
                    .expect("ai.tts")
                    .as_object()
                    .expect("object")
                    .len(),
                1,
                "{provider}: ai.tts holds routing only"
            );
        }

        // STT: whisper (the only built-in STT provider).
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": { "stt": { "provider": "whisper", "model": "ggml-small.bin", "language": "en" } },
            "plugins": { "list": { "whisper": { "enable": true, "config": null } } }
        });
        migrate_v6_to_v7(&mut doc).expect("migration succeeds");
        let config = doc
            .pointer("/plugins/list/whisper/config")
            .expect("whisper config");
        assert_eq!(
            config.get("model"),
            Some(&serde_json::json!("ggml-small.bin"))
        );
        assert_eq!(config.get("language"), Some(&serde_json::json!("en")));
    }

    /// The stock v6 default (`config: null` + `ai.tts` values) migrates
    /// without failing: the null blob is normalized to an empty object.
    #[test]
    fn v6_to_v7_normalizes_null_plugin_config() {
        let mut doc = serde_json::json!({
            "version": 6,
            "ai": {
                "tts": { "provider": "kokoro", "voice": "af_heart", "speed": 1.0, "language": "ja" }
            },
            "plugins": {
                "list": {
                    "kokoro": { "enable": true, "config": null },
                    "voicevox": { "enable": true, "config": null }
                }
            }
        });
        migrate_v6_to_v7(&mut doc).expect("null config migrates");
        let kokoro = doc
            .pointer("/plugins/list/kokoro/config")
            .expect("kokoro config");
        assert_eq!(kokoro.get("voice"), Some(&serde_json::json!("af_heart")));
        assert_eq!(kokoro.get("speed"), Some(&serde_json::json!(1.0)));
        let voicevox = doc
            .pointer("/plugins/list/voicevox/config")
            .expect("voicevox config");
        assert_eq!(
            voicevox.get("mode"),
            Some(&serde_json::json!("external")),
            "null voicevox config is normalized and gains mode"
        );
    }
}
