//! Smoke tests for the mind runtime configuration wiring.
//!
//! These tests guard the fix for issue #95: `ene_mind::MindConfig`
//! is declared in the `ene-mind` crate via the `define_config!` macro,
//! which expands to a `#[ctor::ctor(unsafe)] fn register()` that pushes the
//! schema into the global `SCHEMA_REGISTRY`. That `ctor` only fires when
//! the `ene-mind` crate is actually linked into the running binary —
//! i.e. when some downstream crate (`ene-runtime`, `ene-cli`, `ene-desktop`)
//! declares a hard dependency on it.
//!
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use expect/panic for schema wiring assertions"
)]
//!
//! Without this dependency, the JSON schema shipped to users would be
//! missing the `mind` section even though the struct exists in
//! source and the docs (`docs/reference/configuration/settings.md`) already describe
//! it. The tests below assert both halves of the wiring:
//!
//! 1. The generated schema exposes a top-level `mind` property
//!    AND the inner sub-types appear in `$defs` / `definitions`
//!    (proves the `ctor` ran and `generate_schema_json` walked the
//!    registry correctly).
//! 2. `EneConfig::get_section::<MindConfig>()` returns the
//!    macro-defined defaults when the key is absent (proves the
//!    deserialisation path works against `EneConfig::extra`).
//!
//! Note: `ene-mind` is a non-optional dependency of `ene-runtime` as of
//! the fix, so simply compiling this test binary is enough to link the
//! crate and trigger the `ctor`.

use ene_config::EneConfig;
use ene_mind::{ContextConfig, EmotionConfig, MindConfig, MindMemoryConfig};

#[test]
fn mind_schema_appears_as_top_level_property_when_ene_mind_is_linked() {
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");

    let properties = value
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("schema must expose top-level properties map");

    let mind_prop = properties.get("mind").unwrap_or_else(|| {
        panic!(
            "expected `mind` to be a top-level property after linking \
                 `ene-mind`; got property keys: {:?}",
            properties.keys().cloned().collect::<Vec<_>>()
        )
    });

    // The registered `MindConfig` schema should be a plain object
    // with nested `context` / `memory` / `emotion` / `character` sub-properties.
    let mind_properties = mind_prop
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("`mind` property should be a struct with sub-properties");
    for sub in ["context", "memory", "emotion", "character"] {
        assert!(
            mind_properties.contains_key(sub),
            "mind.{sub} should appear in the registered schema; got keys: {:?}",
            mind_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
    assert!(
        !mind_properties.contains_key("enabled"),
        "removed mind.enabled must not appear in the schema"
    );
}

#[test]
fn mind_subtypes_appear_in_schema_definitions() {
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");

    let defs = value
        .get("$defs")
        .or_else(|| value.get("definitions"))
        .expect("schema must expose a definitions map");

    // `MindConfig` is the *root* of the registered entry, so it
    // appears as a top-level property (covered by the sibling test).
    // The sub-types — `ContextConfig`, `MindMemoryConfig`,
    // `EmotionConfig`, `CharacterMemoryConfig`, and the `EngineMode`
    // enum used by `emotion.engine` — are pulled in as referenced
    // types and must end up in the `$defs` map.
    for sub_type in [
        "ContextConfig",
        "MindMemoryConfig",
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
fn mind_section_is_absent_from_default_ene_config() {
    // Sanity check: the on-disk `settings.json` shipped with the project
    // does not contain a `mind` block (it falls through to the
    // macro-defined defaults). If a future change starts emitting
    // `mind` into `EneConfig::default()`, downstream test
    // `mind_section_defaults_match_macro_definition` still passes
    // because it compares against the macro default, but reviewers
    // should know that the `extra` map is no longer empty.
    let cfg = EneConfig::default();
    assert!(
        !cfg.extra.contains_key("mind"),
        "EneConfig::default() should not pre-populate `mind`; \
         the field is read on-demand via `get_section`"
    );
}

#[test]
fn mind_section_defaults_match_macro_definition() {
    // Reproduce the defaults defined in `ene-mind::config.rs`. If
    // those defaults change, this test must change in lockstep — that
    // is the point: the test pins the contract documented in
    // `docs/reference/configuration/settings.md`.
    let cfg = EneConfig::default();
    let mind: MindConfig = cfg
        .get_section::<MindConfig>()
        .expect("settings-target section should always be retrievable");
    assert_eq!(
        mind.context,
        ContextConfig::default(),
        "mind.context should equal the macro-defined defaults"
    );
    assert_eq!(
        mind.emotion,
        EmotionConfig::default(),
        "mind.emotion should equal the macro-defined defaults"
    );
    assert_eq!(
        mind.memory,
        MindMemoryConfig::default(),
        "mind.memory should equal the macro-defined defaults"
    );
    assert!(
        mind.context.max_prompt_tokens > 0,
        "mind.context.max_prompt_tokens must be a positive budget"
    );
    assert_eq!(
        mind.context.max_prompt_tokens, 12_000,
        "mind.context.max_prompt_tokens should be 12_000 per docs"
    );
    assert_eq!(
        mind.context.recent_turns, 8,
        "mind.context.recent_turns should be 8 per docs"
    );
    assert_eq!(
        mind.memory.recall_result_limit, 8,
        "mind.memory.recall_result_limit should be 8 per docs"
    );
    assert!(
        !mind.memory.use_hyde,
        "mind.memory.use_hyde should default to false"
    );
    assert!(
        (mind.memory.hyde_blend - 0.6).abs() < f32::EPSILON,
        "mind.memory.hyde_blend should default to 0.6"
    );
    assert!(
        mind.memory.mmr_enabled,
        "mind.memory.mmr_enabled should default to true"
    );
    assert!(
        (mind.memory.mmr_lambda - 0.7).abs() < f32::EPSILON,
        "mind.memory.mmr_lambda should default to 0.7"
    );
    assert!(
        (mind.memory.mmr_duplicate_cluster_threshold - 0.75).abs() < f32::EPSILON,
        "mind.memory.mmr_duplicate_cluster_threshold should default to 0.75"
    );
    assert_eq!(
        mind.memory.mmr_min_slots_semantic, 1,
        "mind.memory.mmr_min_slots_semantic should default to 1"
    );
    assert_eq!(
        mind.memory.mmr_min_slots_episodic, 1,
        "mind.memory.mmr_min_slots_episodic should default to 1"
    );
    assert_eq!(
        mind.memory.mmr_min_slots_user_profile, 1,
        "mind.memory.mmr_min_slots_user_profile should default to 1"
    );
    assert_eq!(
        mind.memory.mmr_min_slots_commitment, 1,
        "mind.memory.mmr_min_slots_commitment should default to 1"
    );
    assert!(
        (mind.memory.mmr_source_diversity_bonus - 0.05).abs() < f32::EPSILON,
        "mind.memory.mmr_source_diversity_bonus should default to 0.05"
    );
}

#[test]
fn mind_section_round_trips_through_ene_config_extra() {
    // Write a custom mind block, load it back, and confirm the
    // sub-fields survive. This proves the section path inside `extra`
    // (key `mind` → nested object) is fully wired.
    let mut cfg = EneConfig::default();
    let custom = MindConfig {
        context: ContextConfig {
            max_prompt_tokens: 16_384,
            recent_turns: 12,
            ..ContextConfig::default()
        },
        ..MindConfig::default()
    };
    cfg.set_section(&custom)
        .expect("set_section should succeed for settings-target");

    // Serialise → reparse → read back
    let json = serde_json::to_string(&cfg).expect("serialise EneConfig");
    let reparsed: EneConfig = serde_json::from_str(&json).expect("reparse EneConfig");

    let loaded: MindConfig = reparsed
        .get_section::<MindConfig>()
        .expect("mind should be retrievable after round-trip");
    assert_eq!(loaded.context.max_prompt_tokens, 16_384);
    assert_eq!(loaded.context.recent_turns, 12);
}

#[test]
fn mind_section_survives_in_serialised_settings_json() {
    // Validate the end-to-end shape: a `settings.json` produced by
    // serialising `EneConfig` with a populated mind block must be
    // valid JSON Schema-acceptable input (i.e. the generated schema
    // would accept it). We assert both that the section shows up in
    // the JSON and that the schema's `properties.mind` matches.
    let mut cfg = EneConfig::default();
    let custom = MindConfig::default();
    cfg.set_section(&custom)
        .expect("set_section should succeed");

    let json = serde_json::to_value(&cfg).expect("serialise EneConfig");
    // `EneConfig::extra` is `#[serde(flatten)]`, so the `mind`
    // block appears at the top level of the serialised JSON, not
    // under an `extra` key.
    let mind = json
        .get("mind")
        .and_then(|v| v.as_object())
        .expect("`mind` should appear as a top-level object after set_section");

    // The defaults are documented in `docs/reference/configuration/settings.md`;
    // pin the contract here as well.
    assert_eq!(
        mind.get("context").and_then(|v| v.get("max_prompt_tokens")),
        Some(&serde_json::json!(12_000))
    );

    // The same shape should be visible in the generated schema.
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let schema_value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");
    let schema_mind = schema_value
        .get("properties")
        .and_then(|p| p.get("mind"))
        .expect("schema should expose top-level `mind`");
    assert!(
        schema_mind
            .get("properties")
            .and_then(|p| p.get("context"))
            .is_some(),
        "schema should report `mind.context`"
    );
    assert!(
        schema_mind
            .get("properties")
            .and_then(|p| p.get("enabled"))
            .is_none(),
        "schema must not expose removed `mind.enabled`"
    );
}

#[test]
fn mind_section_is_present_in_written_settings_schema_file() {
    // End-to-end check: write the schema to the real on-disk
    // `assets/schema/settings.schema.json` (this file is gitignored
    // per AGENTS.md §4.2 and is auto-regenerated by the CLI on
    // startup, so mutating it from a test is intentional and safe)
    // and confirm the `mind` section made it through.
    //
    // This is a regression guard for issue #95. If a future change
    // removes `ene-mind` from `ene-runtime`'s dependencies, the
    // `ctor::ctor(unsafe) fn register()` from `define_config!` will
    // stop firing, the registry will no longer contain
    // `MindConfig`, and the on-disk schema will silently lose
    // the `mind` block — the docs would then describe a section
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
    let mind = properties.get("mind").unwrap_or_else(|| {
        panic!(
            "on-disk settings.schema.json is missing top-level `mind`; \
             property keys: {:?}",
            properties.keys().cloned().collect::<Vec<_>>()
        )
    });
    let mind_properties = mind
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("`mind` property should be a struct with sub-properties");
    for sub in ["context", "memory", "emotion", "character"] {
        assert!(
            mind_properties.contains_key(sub),
            "on-disk schema: `mind.{sub}` should be present; got keys: {:?}",
            mind_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
    assert!(
        !mind_properties.contains_key("enabled"),
        "on-disk schema must not include removed `mind.enabled`"
    );
}
