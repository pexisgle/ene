//! Smoke tests for the cognitive runtime configuration wiring.
//!
//! These tests guard the fix for issue #95: `ene_cognition::CognitionConfig`
//! is declared in the `ene-cognition` crate via the `define_config!` macro,
//! which expands to a `#[ctor::ctor(unsafe)] fn register()` that pushes the
//! schema into the global `SCHEMA_REGISTRY`. That `ctor` only fires when
//! the `ene-cognition` crate is actually linked into the running binary —
//! i.e. when some downstream crate (`ene-core`, `ene-cli`, `ene-desktop`)
//! declares a hard dependency on it.
//!
//! Without this dependency, the JSON schema shipped to users would be
//! missing the `cognition` section even though the struct exists in
//! source and the docs (`docs/configuration/settings.md`) already describe
//! it. The tests below assert both halves of the wiring:
//!
//! 1. The generated schema exposes a top-level `cognition` property
//!    AND the inner sub-types appear in `$defs` / `definitions`
//!    (proves the `ctor` ran and `generate_schema_json` walked the
//!    registry correctly).
//! 2. `EneConfig::get_section::<CognitionConfig>()` returns the
//!    macro-defined defaults when the key is absent (proves the
//!    deserialisation path works against `EneConfig::extra`).
//!
//! Note: `ene-cognition` is a non-optional dependency of `ene-core` as of
//! the fix, so simply compiling this test binary is enough to link the
//! crate and trigger the `ctor`.

use ene_config::EneConfig;
use ene_core::{CognitionConfig, ContextConfig, EmotionConfig};

#[test]
fn cognition_schema_appears_as_top_level_property_when_ene_cognition_is_linked() {
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");

    let properties = value
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("schema must expose top-level properties map");

    let cognition_prop = properties.get("cognition").unwrap_or_else(|| {
        panic!(
            "expected `cognition` to be a top-level property after linking \
                 `ene-cognition`; got property keys: {:?}",
            properties.keys().cloned().collect::<Vec<_>>()
        )
    });

    // The registered `CognitionConfig` schema should be a plain object
    // with an `enabled` field (its top-level bool) and nested
    // `context` / `memory` / `emotion` / `character` sub-properties.
    let cog_properties = cognition_prop
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("`cognition` property should be a struct with sub-properties");
    for sub in ["enabled", "context", "memory", "emotion", "character"] {
        assert!(
            cog_properties.contains_key(sub),
            "cognition.{sub} should appear in the registered schema; got keys: {:?}",
            cog_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
}

#[test]
fn cognition_subtypes_appear_in_schema_definitions() {
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");

    let defs = value
        .get("$defs")
        .or_else(|| value.get("definitions"))
        .expect("schema must expose a definitions map");

    // `CognitionConfig` is the *root* of the registered entry, so it
    // appears as a top-level property (covered by the sibling test).
    // The sub-types — `ContextConfig`, `CognitionMemoryConfig`,
    // `EmotionConfig`, `CharacterMemoryConfig`, and the `EngineMode`
    // enum used by `emotion.engine` — are pulled in as referenced
    // types and must end up in the `$defs` map.
    for sub_type in [
        "ContextConfig",
        "CognitionMemoryConfig",
        "EmotionConfig",
        "CharacterMemoryConfig",
        "EngineMode",
    ] {
        assert!(
            defs.get(sub_type).is_some(),
            "expected `{sub_type}` to be registered as a referenced sub-type; got defs keys: {:?}",
            defs.as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        );
    }
}

#[test]
fn cognition_section_is_absent_from_default_ene_config() {
    // Sanity check: the on-disk `settings.json` shipped with the project
    // does not contain a `cognition` block (it falls through to the
    // macro-defined defaults). If a future change starts emitting
    // `cognition` into `EneConfig::default()`, downstream test
    // `cognition_section_defaults_match_macro_definition` still passes
    // because it compares against the macro default, but reviewers
    // should know that the `extra` map is no longer empty.
    let cfg = EneConfig::default();
    assert!(
        !cfg.extra.contains_key("cognition"),
        "EneConfig::default() should not pre-populate `cognition`; \
         the field is read on-demand via `get_section`"
    );
}

#[test]
fn cognition_section_defaults_match_macro_definition() {
    // Reproduce the defaults defined in `ene-cognition::config.rs`. If
    // those defaults change, this test must change in lockstep — that
    // is the point: the test pins the contract documented in
    // `docs/configuration/settings.md`.
    let cfg = EneConfig::default();
    let cog: CognitionConfig = cfg
        .get_section::<CognitionConfig>()
        .expect("settings-target section should always be retrievable");

    assert!(cog.enabled, "cognition.enabled should default to true");
    assert_eq!(
        cog.context,
        ContextConfig::default(),
        "cognition.context should equal the macro-defined defaults"
    );
    assert_eq!(
        cog.emotion,
        EmotionConfig::default(),
        "cognition.emotion should equal the macro-defined defaults"
    );
    assert!(
        cog.context.max_prompt_tokens > 0,
        "cognition.context.max_prompt_tokens must be a positive budget"
    );
    assert_eq!(
        cog.context.max_prompt_tokens, 12_000,
        "cognition.context.max_prompt_tokens should be 12_000 per docs"
    );
    assert_eq!(
        cog.context.recent_turns, 8,
        "cognition.context.recent_turns should be 8 per docs"
    );
}

#[test]
fn cognition_section_round_trips_through_ene_config_extra() {
    // Write a custom cognition block, load it back, and confirm the
    // sub-fields survive. This proves the section path inside `extra`
    // (key `cognition` → nested object) is fully wired.
    let mut cfg = EneConfig::default();
    let custom = CognitionConfig {
        enabled: false,
        context: ContextConfig {
            max_prompt_tokens: 16_384,
            recent_turns: 12,
            ..ContextConfig::default()
        },
        ..CognitionConfig::default()
    };
    cfg.set_section(&custom)
        .expect("set_section should succeed for settings-target");

    // Serialise → reparse → read back
    let json = serde_json::to_string(&cfg).expect("serialise EneConfig");
    let reparsed: EneConfig = serde_json::from_str(&json).expect("reparse EneConfig");

    let loaded: CognitionConfig = reparsed
        .get_section::<CognitionConfig>()
        .expect("cognition should be retrievable after round-trip");
    assert!(
        !loaded.enabled,
        "cognition.enabled should round-trip as false"
    );
    assert_eq!(loaded.context.max_prompt_tokens, 16_384);
    assert_eq!(loaded.context.recent_turns, 12);
}

#[test]
fn cognition_section_survives_in_serialised_settings_json() {
    // Validate the end-to-end shape: a `settings.json` produced by
    // serialising `EneConfig` with a populated cognition block must be
    // valid JSON Schema-acceptable input (i.e. the generated schema
    // would accept it). We assert both that the section shows up in
    // the JSON and that the schema's `properties.cognition` matches.
    let mut cfg = EneConfig::default();
    let custom = CognitionConfig::default();
    cfg.set_section(&custom)
        .expect("set_section should succeed");

    let json = serde_json::to_value(&cfg).expect("serialise EneConfig");
    // `EneConfig::extra` is `#[serde(flatten)]`, so the `cognition`
    // block appears at the top level of the serialised JSON, not
    // under an `extra` key.
    let cognition = json
        .get("cognition")
        .and_then(|v| v.as_object())
        .expect("`cognition` should appear as a top-level object after set_section");

    // The defaults are documented in `docs/configuration/settings.md`;
    // pin the contract here as well.
    assert_eq!(cognition.get("enabled"), Some(&serde_json::json!(true)));
    assert_eq!(
        cognition
            .get("context")
            .and_then(|v| v.get("max_prompt_tokens")),
        Some(&serde_json::json!(12_000))
    );

    // The same shape should be visible in the generated schema.
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let schema_value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");
    let schema_cognition = schema_value
        .get("properties")
        .and_then(|p| p.get("cognition"))
        .expect("schema should expose top-level `cognition`");
    assert_eq!(
        schema_cognition
            .get("properties")
            .and_then(|p| p.get("enabled"))
            .and_then(|e| e.get("default")),
        Some(&serde_json::json!(true)),
        "schema should report `cognition.enabled` default = true"
    );
}

#[test]
fn cognition_section_is_present_in_written_settings_schema_file() {
    // End-to-end check: write the schema to the real on-disk
    // `assets/schema/settings.schema.json` (this file is gitignored
    // per AGENTS.md §4.2 and is auto-regenerated by the CLI on
    // startup, so mutating it from a test is intentional and safe)
    // and confirm the `cognition` section made it through.
    //
    // This is a regression guard for issue #95. If a future change
    // removes `ene-cognition` from `ene-core`'s dependencies, the
    // `ctor::ctor(unsafe) fn register()` from `define_config!` will
    // stop firing, the registry will no longer contain
    // `CognitionConfig`, and the on-disk schema will silently lose
    // the `cognition` block — the docs would then describe a section
    // the schema doesn't validate.
    ene_config::write_schemas(&ene_config::paths::assets_dir());

    let schema_path = ene_config::paths::assets_dir()
        .join("schema")
        .join("settings.schema.json");
    let raw = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", schema_path.display()));
    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("on-disk schema should be valid JSON");

    let properties = value
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("on-disk schema must expose top-level properties map");
    let cognition = properties.get("cognition").unwrap_or_else(|| {
        panic!(
            "on-disk settings.schema.json is missing top-level `cognition`; \
             property keys: {:?}",
            properties.keys().cloned().collect::<Vec<_>>()
        )
    });
    let cog_properties = cognition
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("`cognition` property should be a struct with sub-properties");
    for sub in ["enabled", "context", "memory", "emotion", "character"] {
        assert!(
            cog_properties.contains_key(sub),
            "on-disk schema: `cognition.{sub}` should be present; got keys: {:?}",
            cog_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
}
