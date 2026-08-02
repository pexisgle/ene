//! Integration tests for the mind runtime configuration wiring.
//!
//! Public `MindConfig` exposes `emotion`, `proactive`, and `memory_limits`;
//! `context` / `memory` / `character` remain code defaults (serde-skipped).
//!
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use expect/panic for schema wiring assertions"
)]

use ene_config::EneConfig;
use ene_mind::{
    ContextConfig, EmotionConfig, MindConfig, MindMemoryConfig, MindMemoryLimitsConfig,
};

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

    let mind_properties = mind_prop
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("`mind` property should be a struct with sub-properties");
    for sub in ["emotion", "proactive", "memory_limits"] {
        assert!(
            mind_properties.contains_key(sub),
            "mind.{sub} should appear in the registered schema; got keys: {:?}",
            mind_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
    for hidden in ["context", "memory", "character", "enabled"] {
        assert!(
            !mind_properties.contains_key(hidden),
            "internal/removed mind.{hidden} must not appear in the schema; got keys: {:?}",
            mind_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
}

#[test]
fn mind_public_subtypes_appear_in_schema_definitions() {
    let schema_json =
        ene_config::generate_schema_json().expect("schema generation should not fail");
    let value: serde_json::Value =
        serde_json::from_str(&schema_json).expect("schema should be valid JSON");

    let defs = value
        .get("$defs")
        .or_else(|| value.get("definitions"))
        .expect("schema must expose a definitions map");

    for sub_type in ["EmotionConfig", "ProactiveConfig", "MindMemoryLimitsConfig"] {
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
    let cfg = EneConfig::default();
    assert!(
        !cfg.extra.contains_key("mind"),
        "EneConfig::default() should not pre-populate `mind`; \
         the field is read on-demand via `get_section`"
    );
}

#[test]
fn mind_section_defaults_match_macro_definition() {
    let cfg = EneConfig::default();
    let mind: MindConfig = cfg
        .get_section::<MindConfig>()
        .expect("settings-target section should always be retrievable");
    assert_eq!(
        mind.context,
        ContextConfig::default(),
        "mind.context should equal the code defaults"
    );
    assert_eq!(
        mind.emotion,
        EmotionConfig::default(),
        "mind.emotion should equal the macro-defined defaults"
    );
    assert_eq!(
        mind.memory,
        MindMemoryConfig::default(),
        "mind.memory should equal the code defaults"
    );
    assert_eq!(
        mind.memory_limits,
        MindMemoryLimitsConfig::default(),
        "mind.memory_limits should equal the code defaults"
    );
    assert_eq!(mind.context.max_prompt_tokens, None);
    assert!(mind.emotion.enabled);
    assert!(!mind.proactive.enabled);
    assert_eq!(mind.proactive.interval_seconds, 60);
    assert_eq!(mind.proactive.min_idle_seconds, 120);
    assert_eq!(mind.proactive.cooldown_seconds, 300);
}

#[test]
fn mind_section_round_trips_public_fields_only() {
    let mut cfg = EneConfig::default();
    let mut custom = MindConfig::default();
    custom.proactive.enabled = true;
    custom.proactive.interval_seconds = 90;
    custom.emotion.enabled = false;
    custom.memory_limits.commitment_active_match_limit = 128;
    // Mutating skipped fields must not survive JSON round-trip.
    custom.context.max_prompt_tokens = Some(16_384);
    custom.memory.recall_result_limit = 999;
    cfg.set_section(&custom)
        .expect("set_section should succeed for settings-target");

    let json = serde_json::to_string(&cfg).expect("serialise EneConfig");
    let reparsed: EneConfig = serde_json::from_str(&json).expect("reparse EneConfig");

    let loaded: MindConfig = reparsed
        .get_section::<MindConfig>()
        .expect("mind should be retrievable after round-trip");
    assert!(loaded.proactive.enabled);
    assert_eq!(loaded.proactive.interval_seconds, 90);
    assert!(!loaded.emotion.enabled);
    assert_eq!(
        loaded.memory_limits.commitment_active_match_limit, 128,
        "the public memory_limits field must survive the JSON round-trip"
    );
    assert_eq!(
        loaded.memory,
        MindMemoryConfig::default(),
        "the hidden memory section stays at code defaults on deserialize"
    );
    assert_eq!(
        loaded.context.max_prompt_tokens,
        ContextConfig::default().max_prompt_tokens,
        "skipped context fields reset to code defaults on deserialize"
    );
}

#[test]
fn mind_section_survives_in_serialised_settings_json() {
    let mut cfg = EneConfig::default();
    let mut custom = MindConfig::default();
    custom.proactive.enabled = true;
    cfg.set_section(&custom)
        .expect("set_section should succeed");

    let json = serde_json::to_value(&cfg).expect("serialise EneConfig");
    let mind = json
        .get("mind")
        .and_then(|v| v.as_object())
        .expect("`mind` should appear as a top-level object after set_section");

    assert!(
        mind.get("context").is_none(),
        "context must not be serialized"
    );
    assert!(
        mind.get("memory").is_none(),
        "memory must not be serialized — it is a code-defaulted section"
    );
    assert!(
        mind.get("memory_limits").is_some(),
        "memory_limits must be serialized as the public memory surface"
    );
    assert_eq!(
        mind.get("proactive").and_then(|v| v.get("enabled")),
        Some(&serde_json::json!(true))
    );

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
            .and_then(|p| p.get("proactive"))
            .is_some(),
        "schema should report `mind.proactive`"
    );
    assert!(
        schema_mind
            .get("properties")
            .and_then(|p| p.get("memory_limits"))
            .is_some(),
        "schema should report `mind.memory_limits`"
    );
    assert!(
        schema_mind
            .get("properties")
            .and_then(|p| p.get("memory"))
            .is_none(),
        "schema must not expose internal `mind.memory`"
    );
    assert!(
        schema_mind
            .get("properties")
            .and_then(|p| p.get("context"))
            .is_none(),
        "schema must not expose internal `mind.context`"
    );
}

#[test]
fn mind_section_is_present_in_written_settings_schema_file() {
    ene_config::write_schemas(ene_config::paths::assets_dir());

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
    for sub in ["emotion", "proactive", "memory_limits"] {
        assert!(
            mind_properties.contains_key(sub),
            "on-disk schema: `mind.{sub}` should be present; got keys: {:?}",
            mind_properties.keys().cloned().collect::<Vec<_>>()
        );
    }
    for hidden in ["context", "memory", "character"] {
        assert!(
            !mind_properties.contains_key(hidden),
            "on-disk schema must not include internal `mind.{hidden}`"
        );
    }
}
