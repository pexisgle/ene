//! Phase 6 context management tests (#79, #81).

use ene_cognition::{
    CognitionConfig, CompressionResult, CompressionLevel, ContextConfig, ContextManager,
    PackInput, compression_has_usable_summary, evaluate_compression_trigger, pack_prompt,
    validate_context_config, ContextBudget,
};
use ene_cognition::character::IdentityKernel;

#[test]
fn context_config_validation_accepts_default_budgets() {
    assert!(validate_context_config(&ContextConfig::default()).is_ok());
}

#[test]
fn compression_trigger_respects_enabled_flag() {
    let mut config = ContextConfig::default();
    config.compression_enabled = false;
    assert!(evaluate_compression_trigger(&config, 100, 100).is_none());
}

#[test]
fn context_manager_validates_config() {
    assert!(ContextManager::validate_config(&ContextConfig::default()).is_ok());
}

#[test]
fn cognition_engine_validates_config_on_compose() {
    let config = CognitionConfig {
        context: ContextConfig {
            max_prompt_tokens: 100,
            scene_summary_tokens: 800,
            ..ContextConfig::default()
        },
        ..CognitionConfig::default()
    };
    assert!(ene_cognition::CognitionEngine::validate_config(&config).is_err());
}

#[test]
fn compression_without_summary_is_not_usable() {
    let result = CompressionResult {
        session_id: "sess".into(),
        span_id: 1,
        summary: String::new(),
        level: CompressionLevel::Scene,
    };
    assert!(!compression_has_usable_summary(&result));
}

#[test]
fn pack_prompt_counts_history_toward_total_budget() {
    let config = ContextConfig {
        max_prompt_tokens: 35,
        recent_turns: 4,
        ..ContextConfig::default()
    };
    let budget = ContextBudget::from_config(&config);
    let packed = pack_prompt(
        PackInput {
            platform_contract: None,
            identity_kernel: IdentityKernel {
                name: "Ene".into(),
                text: "K".into(),
                post_history_instructions: None,
            },
            behavior_contract: None,
            style_examples: vec![],
            recalled: vec![],
            commitments: vec![],
            affect_summary: None,
            scene_summary: None,
            history: vec![
                ene_cognition::HistoryEntry {
                    role: "user".into(),
                    content: "x".repeat(80),
                },
                ene_cognition::HistoryEntry {
                    role: "assistant".into(),
                    content: "y".repeat(80),
                },
                ene_cognition::HistoryEntry {
                    role: "user".into(),
                    content: "recent".into(),
                },
                ene_cognition::HistoryEntry {
                    role: "assistant".into(),
                    content: "latest".into(),
                },
            ],
            output_contract: None,
            user_input: "hi".into(),
        },
        &budget,
    );
    assert!(packed.meta.packed_tokens <= budget.total_tokens);
}
