#![expect(
    clippy::expect_used,
    reason = "integration tests use expect for concise assertions"
)]

use std::sync::Arc;
use std::time::Duration;

use ene_mind::character::{IdentityKernel, LorebookInjection};
use ene_mind::{
    CompressionLevel, CompressionResult, ContextBudget, ContextConfig, HistoryEntry, PackInput,
    PromptSectionKind, compression_has_usable_summary, evaluate_compression_trigger, pack_prompt,
    plan_retroactive_compression,
};

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
    assert!(matches!(
        evaluate_compression_trigger(&config, 1, 500),
        Some(ene_mind::CompressionReason::ContextPressure {
            history_tokens: 500
        })
    ));
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
    // Packing budgets against a single window (here injected directly)
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
            style_examples: vec![],
            recalled: vec![],
            workspace_documents: vec![],
            commitments: vec![],
            affect_summary: None,
            character_state_note: None,
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
            lorebook: LorebookInjection::default(),
            user_persona: None,
            compression_pending: false,
            user_input: "hi".into(),
            lang: "en".into(),
            now: None,
        },
        &budget,
    );
    assert!(packed.meta.packed_tokens <= budget.total_tokens);
}

/// An alternating user/assistant history of `turns` exchanges, each long
/// enough to dominate the window budgets below (~19 tokens per message).
fn exchanges(turns: usize) -> Vec<HistoryEntry> {
    let mut history = Vec::with_capacity(turns * 2);
    for i in 0..turns {
        history.push(HistoryEntry {
            role: ene_ai::Role::User,
            content: format!("user message {i} ").repeat(4),
        });
        history.push(HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: format!("assistant reply {i} ").repeat(4),
        });
    }
    history
}

/// A `PackInput` with all optional sections empty, so tests only vary the
/// history / compression-pending / scene-summary fields they care about.
fn base_pack_input() -> PackInput {
    PackInput {
        platform_contract: None,
        identity_kernel: IdentityKernel {
            name: "Ene".into(),
            text: "KERNEL".into(),
            post_history_instructions: None,
        },
        style_examples: vec![],
        recalled: vec![],
        workspace_documents: vec![],
        commitments: vec![],
        affect_summary: None,
        character_state_note: None,
        scene_summary: None,
        history: vec![],
        output_contract: None,
        interruption_note: None,
        authors_note: None,
        lorebook: LorebookInjection::default(),
        user_persona: None,
        compression_pending: false,
        user_input: "hi".into(),
        lang: "en".into(),
        now: None,
    }
}

/// A context config that fires the window-pressure trigger on a ~190
/// token history while keeping the turn-count threshold out of the way.
fn window_pressure_config() -> ContextConfig {
    ContextConfig {
        scene_turn_threshold: 999,
        context_pressure_tokens: 100,
        recent_turns: 2,
        ..ContextConfig::default()
    }
}

/// A summarizer stub whose `chat_completion` returns the configured summary
/// (empty / `None` simulates a failing summary) so compression results are
/// deterministic.
struct TestSummaryLlm {
    /// Summary text returned by `chat_completion`. `None` returns an empty
    /// completion, which [`compression_has_usable_summary`] rejects.
    summary: Option<String>,
    /// When set, `chat_completion` blocks until the gate is notified — a
    /// compression task can then be held in flight deterministically.
    gate: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait::async_trait]
impl ene_ai::LlmProvider for TestSummaryLlm {
    fn name(&self) -> &'static str {
        "test-summary-llm"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[ene_ai::LlmMessage],
        _tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<
                        Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                    > + Send,
            >,
        >,
        ene_ai::LlmProviderError,
    > {
        Err(ene_ai::LlmProviderError::Provider(
            "not used in compression tests".into(),
        ))
    }

    async fn chat_completion(
        &self,
        _messages: &[ene_ai::LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        Ok(ene_ai::LlmCompletion::text_only(
            self.summary.clone().unwrap_or_default(),
        ))
    }
}

#[tokio::test]
async fn window_pressure_detaches_oldest_span_without_waiting_for_summary() {
    let store = Arc::new(
        ene_store::MemoryStore::open_in_memory(4)
            .await
            .expect("store"),
    );
    // The compression task is held on the gate so the "nothing is written
    // until the summary arrives" assertions below cannot race the spawned
    // task (it must not complete before the test says so).
    let gate = Arc::new(tokio::sync::Notify::new());
    let provider: Arc<dyn ene_ai::LlmProvider> = Arc::new(TestSummaryLlm {
        summary: Some("scene summary text".into()),
        gate: Some(gate.clone()),
    });
    let mut manager = ene_mind::ContextManager::default();
    let config = window_pressure_config();
    let history = exchanges(5);

    manager.check_and_trigger(
        &config,
        5,
        &history,
        "sess-371-detach",
        "Ene",
        "user",
        store.clone(),
        provider,
    );
    assert!(
        manager.has_pending(),
        "window pressure must spawn a compression"
    );

    // The gap: the summary has not arrived, so the session history has not
    // shrunk. Packing must hold the window constraint by detaching the oldest
    // span from the prompt-visible history synchronously — shedding no
    // sections and touching no history.
    let budget = ContextBudget::with_capacity(60);
    let packed = pack_prompt(
        PackInput {
            history: history.clone(),
            compression_pending: manager.has_pending(),
            ..base_pack_input()
        },
        &budget,
    );

    assert!(
        packed.meta.history_messages_detached > 0,
        "the oldest span must be detached while the summary is pending"
    );
    assert_eq!(
        packed.meta.history_messages_dropped, 0,
        "detachment must not fall back to the dropping path"
    );
    assert_eq!(
        packed.meta.history_messages_detached + packed.packet.history.len(),
        history.len(),
        "detachment only hides the leading span from the prompt"
    );
    assert!(
        packed.meta.packed_tokens <= budget.total_tokens,
        "the window constraint must hold without waiting for the summary"
    );

    // DB history is NOT modified: the detached span is not summarized or
    // removed — nothing is written until the task completes.
    assert_eq!(
        history.len(),
        10,
        "the session/DB history must be untouched by detachment"
    );
    let spans = store
        .list_memory_spans_by_session_and_level("sess-371-detach", CompressionLevel::Scene.as_i32())
        .await
        .expect("list spans");
    assert!(
        spans.is_empty(),
        "no span is written while the compression task is still in flight"
    );

    // Release the gate: the task now runs to completion and persists the
    // span itself — detachment alone never wrote anything.
    gate.notify_one();
    loop {
        if manager.poll_pending().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let spans = store
        .list_memory_spans_by_session_and_level("sess-371-detach", CompressionLevel::Scene.as_i32())
        .await
        .expect("list spans");
    assert_eq!(
        spans.len(),
        1,
        "the compression task wrote the span on completion"
    );
    assert_eq!(
        spans[0].compressed_summary.as_deref(),
        Some("scene summary text")
    );
}

#[tokio::test]
async fn summary_arrival_converges_state_without_further_detachment() {
    let store = Arc::new(
        ene_store::MemoryStore::open_in_memory(4)
            .await
            .expect("store"),
    );
    let provider: Arc<dyn ene_ai::LlmProvider> = Arc::new(TestSummaryLlm {
        summary: Some("They discussed the plan.".into()),
        gate: None,
    });
    let mut manager = ene_mind::ContextManager::default();
    let config = window_pressure_config();
    let history = exchanges(5);
    let budget = ContextBudget::with_capacity(100);

    manager.check_and_trigger(
        &config,
        5,
        &history,
        "sess-371-converge",
        "Ene",
        "user",
        store.clone(),
        provider,
    );
    assert!(manager.has_pending());

    // Gap turn: the prompt must detach to fit while the summary is pending.
    let gap_packed = pack_prompt(
        PackInput {
            history: history.clone(),
            compression_pending: manager.has_pending(),
            ..base_pack_input()
        },
        &budget,
    );
    assert!(
        gap_packed.meta.history_messages_detached > 0,
        "the gap turn must detach the oldest span"
    );

    // The summary arrives (what `apply_pending_compression` polls).
    let result = loop {
        if let Some(polled) = manager.poll_pending() {
            break polled.expect("compression task succeeds");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(compression_has_usable_summary(&result));

    // The actor applies it by trimming the session history to the recent
    // window; the summary is served as the scene section where the
    // detached span used to be.
    let recent_cap = config.recent_turns.saturating_mul(2).max(2);
    let trimmed: Vec<HistoryEntry> = history[history.len() - recent_cap..].to_vec();

    let converged = pack_prompt(
        PackInput {
            scene_summary: Some(result.summary.clone()),
            history: trimmed.clone(),
            compression_pending: false,
            ..base_pack_input()
        },
        &budget,
    );

    assert_eq!(
        converged.meta.history_messages_detached, 0,
        "once the summary is applied the prompt no longer detaches"
    );
    assert!(
        converged.meta.packed_tokens <= budget.total_tokens,
        "the window constraint holds after convergence"
    );
    let scene = converged
        .packet
        .section(PromptSectionKind::SceneState)
        .expect("scene summary section present");
    assert!(
        scene.content.contains(&result.summary),
        "the summary is inserted where the detached span was"
    );

    // The span is persisted and serves as the active scene summary.
    let spans = store
        .list_memory_spans_by_session_and_level(
            "sess-371-converge",
            CompressionLevel::Scene.as_i32(),
        )
        .await
        .expect("list spans");
    assert_eq!(spans.len(), 1, "one compressed span after convergence");
    assert_eq!(
        spans[0].compressed_summary.as_deref(),
        Some(result.summary.as_str())
    );
}

#[tokio::test]
async fn repeated_summary_failures_do_not_erase_history() {
    let store = Arc::new(
        ene_store::MemoryStore::open_in_memory(4)
            .await
            .expect("store"),
    );
    let provider: Arc<dyn ene_ai::LlmProvider> = Arc::new(TestSummaryLlm {
        summary: None,
        gate: None,
    });
    let mut manager = ene_mind::ContextManager::default();
    let config = window_pressure_config();
    let source = exchanges(5);
    let budget = ContextBudget::with_capacity(60);

    // Three turns in a row where the summary keeps failing (timeout etc.): each
    // turn the prompt detaches the oldest span from its own copy, but the
    // source history is never erased and the one-exchange floor always
    // survives — no infinite detachment.
    for turn in 0..3 {
        manager.check_and_trigger(
            &config,
            5 + turn,
            &source,
            "sess-371-fail",
            "Ene",
            "user",
            store.clone(),
            provider.clone(),
        );
        assert!(manager.has_pending(), "turn {turn}: pressure keeps firing");

        let packed = pack_prompt(
            PackInput {
                history: source.clone(),
                compression_pending: manager.has_pending(),
                ..base_pack_input()
            },
            &budget,
        );
        assert!(
            packed.meta.history_messages_detached > 0,
            "turn {turn}: the prompt stays detached while the summary fails"
        );
        assert!(
            packed.packet.history.len() >= 2,
            "turn {turn}: the most recent span always survives"
        );
        assert_eq!(
            packed.meta.history_messages_detached + packed.packet.history.len(),
            source.len(),
            "turn {turn}: detachment is prompt-only"
        );

        // Drain the failed task (no usable summary → history preserved), then
        // the next turn triggers again.
        loop {
            if let Some(polled) = manager.poll_pending() {
                let result = polled.expect("compression task runs");
                assert!(
                    !compression_has_usable_summary(&result),
                    "turn {turn}: the stub must keep failing"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert_eq!(
        source.len(),
        10,
        "repeated summary failures must never erase the history"
    );
}
