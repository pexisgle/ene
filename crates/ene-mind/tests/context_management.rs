//! Context management integration tests (compression, prompt packing, config validation).
#![expect(
    clippy::expect_used,
    reason = "integration tests use expect for concise assertions"
)]

use ene_mind::character::IdentityKernel;
use ene_mind::{
    CompressionLevel, CompressionResult, ContextBudget, ContextConfig, ContextManager, MindConfig,
    PackInput, compression_has_usable_summary, evaluate_compression_trigger, pack_prompt,
    plan_retroactive_compression, validate_context_config,
};

#[test]
fn context_config_validation_accepts_default_budgets() {
    assert!(validate_context_config(&ContextConfig::default()).is_ok());
}

#[test]
fn compression_trigger_fires_on_turn_threshold() {
    let config = ContextConfig::default();
    let reason = evaluate_compression_trigger(&config, config.scene_turn_threshold, 0);
    assert!(matches!(
        reason,
        Some(ene_mind::CompressionReason::TurnThreshold { .. })
    ));
}

#[test]
fn compression_trigger_is_token_based_not_message_count() {
    let config = ContextConfig {
        scene_turn_threshold: 999,
        context_pressure_tokens: 500,
        ..ContextConfig::default()
    };
    // At/above the token ceiling: window-pressure trigger fires.
    assert!(matches!(
        evaluate_compression_trigger(&config, 1, 500),
        Some(ene_mind::CompressionReason::ContextPressure {
            history_tokens: 500
        })
    ));
    // Below the ceiling: nothing fires regardless of message count.
    assert!(evaluate_compression_trigger(&config, 1, 499).is_none());
}

#[test]
fn retroactive_plan_keeps_boundary_turn_and_compresses_prior_span() {
    use ene_mind::HistoryEntry;
    let history: Vec<HistoryEntry> = (0..3)
        .flat_map(|i| {
            [
                HistoryEntry {
                    role: ene_ai::Role::User,
                    content: format!("user {i}"),
                },
                HistoryEntry {
                    role: ene_ai::Role::Assistant,
                    content: format!("assistant {i}"),
                },
            ]
        })
        .collect();
    let plan = plan_retroactive_compression(&history, 3).expect("pre-boundary span");
    assert_eq!(
        plan.turns.len(),
        4,
        "compresses the two pre-boundary exchanges"
    );
    assert_eq!(plan.drop_leading, 4, "drops the pre-boundary prefix");
    assert_eq!((plan.turn_start, plan.turn_end), (0, 2));
}

#[test]
fn retroactive_plan_is_none_for_first_topic() {
    use ene_mind::HistoryEntry;
    let history = vec![
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "hello".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: "hi there".into(),
        },
    ];
    assert!(plan_retroactive_compression(&history, 1).is_none());
}

#[test]
fn context_manager_validates_config() {
    assert!(ContextManager::validate_config(&ContextConfig::default()).is_ok());
}

#[test]
fn cognition_engine_validates_config_on_compose() {
    let config = MindConfig {
        context: ContextConfig {
            max_prompt_tokens: 100,
            scene_summary_tokens: 800,
            ..ContextConfig::default()
        },
        ..MindConfig::default()
    };
    assert!(ene_mind::CognitionEngine::validate_config(&config).is_err());
}

#[test]
fn compression_without_summary_is_not_usable() {
    let result = CompressionResult {
        session_id: "sess".into(),
        span_id: 1,
        summary: String::new(),
        level: CompressionLevel::Scene,
        drop_leading: None,
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
                ene_mind::HistoryEntry {
                    role: ene_ai::Role::User,
                    content: "x".repeat(80),
                },
                ene_mind::HistoryEntry {
                    role: ene_ai::Role::Assistant,
                    content: "y".repeat(80),
                },
                ene_mind::HistoryEntry {
                    role: ene_ai::Role::User,
                    content: "recent".into(),
                },
                ene_mind::HistoryEntry {
                    role: ene_ai::Role::Assistant,
                    content: "latest".into(),
                },
            ],
            output_contract: None,
            interruption_note: None,
            authors_note: None,
            user_persona: None,
            user_input: "hi".into(),
        },
        &budget,
    );
    assert!(packed.meta.packed_tokens <= budget.total_tokens);
}
