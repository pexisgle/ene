//! Context management integration tests (compression, prompt packing).

use ene_mind::character::IdentityKernel;
use ene_mind::{
    CompressionLevel, CompressionResult, ContextBudget, ContextConfig, PackInput,
    compression_has_usable_summary, evaluate_compression_trigger, pack_prompt,
};

#[test]
fn compression_trigger_fires_on_turn_threshold() {
    let config = ContextConfig::default();
    let reason = evaluate_compression_trigger(&config, config.scene_turn_threshold, 4);
    assert!(matches!(
        reason,
        Some(ene_mind::CompressionReason::TurnThreshold { .. })
    ));
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
    // #370: packing budgets against a single window (here injected directly)
    // and trims the oldest history to fit it.
    let budget = ContextBudget::with_capacity(35);
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
