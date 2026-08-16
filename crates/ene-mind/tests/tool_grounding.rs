#![expect(
    clippy::expect_used,
    reason = "integration tests use expect for store setup assertions"
)]

use ene_mind::memory_writer::MemoryWriteProviders;
use ene_mind::memory_writer::MemoryWriter;
use ene_mind::memory_writer::candidate::{ToolResultSummary, TurnInput};
use ene_mind::memory_writer::tool_grounding::{extract_tool_candidates, summarize_tool_result};
use ene_mind::{MindConfig, PostTurnInput};
use ene_store::{AffectState, MemoryKind, MemoryStore};

#[test]
fn summarize_tool_result_truncates_large_output() {
    let big = "x".repeat(10_000);
    let summary = summarize_tool_result("fs", &big, true, 500);
    assert!(summary.summary.chars().count() <= 503);
}

#[test]
fn summarize_tool_result_masks_screenshot_payload() {
    let screenshot = r#"{"type":"screenshot","data":"AAAA"}"#;
    let summary = summarize_tool_result("browser", screenshot, true, 500);
    assert_eq!(
        summary.summary,
        "[Screenshot successfully captured and sent to vision system]"
    );
}

#[test]
fn failed_tool_yields_reflection_candidate() {
    let cfg = MindConfig::default();
    let tool_results = vec![ToolResultSummary {
        tool_name: "web".to_string(),
        success: false,
        summary: "error: timeout".to_string(),
    }];
    let candidates = extract_tool_candidates(&tool_results, &cfg.memory.tool_grounding);
    assert!(
        candidates
            .iter()
            .any(|c| c.kind == MemoryKind::Reflection && c.source_quote.is_empty())
    );
}

#[test]
fn default_tool_grounding_does_not_auto_persist_success() {
    let cfg = MindConfig::default();
    let tool_results = vec![ToolResultSummary {
        tool_name: "fs".to_string(),
        success: true,
        summary: "wrote file.txt".to_string(),
    }];
    let candidates = extract_tool_candidates(&tool_results, &cfg.memory.tool_grounding);
    assert!(
        candidates.is_empty(),
        "successes should not auto-persist by default: {candidates:?}"
    );
}

#[tokio::test]
async fn memory_writer_persists_tool_grounded_procedure_when_enabled() {
    let store = MemoryStore::open_in_memory(4).await.expect("open store");
    let mut config = MindConfig::default();
    config.memory.tool_grounding.persist_success_procedure = true;
    let affect = AffectState::neutral("ene");
    let tools = vec![ToolResultSummary {
        tool_name: "fs".to_string(),
        success: true,
        summary: "wrote file.txt".to_string(),
    }];

    let input = PostTurnInput {
        turn: TurnInput {
            user_message: "save it",
            assistant_message: Some("saved"),
            tool_results: &tools,
        },
        affect: &affect,
        character_id: "ene",
        user_id: "user",
        source_turn: None,
        interrupted: false,
        spoken_text: None,
    };

    MemoryWriter::write_memories(&store, &config, &input, MemoryWriteProviders::default())
        .await
        .expect("write memories");

    let rows = store
        .list_typed_memories_by_source_prefix("ene", "tool:", 16)
        .await
        .expect("list source ref");
    assert!(
        rows.iter().any(|m| m.kind == MemoryKind::Procedure),
        "expected procedure memory from tool grounding, got {rows:?}"
    );
}

#[tokio::test]
async fn memory_writer_persists_tool_grounded_episodic_when_enabled() {
    let store = MemoryStore::open_in_memory(4).await.expect("open store");
    let mut config = MindConfig::default();
    config.memory.min_confidence_to_persist = 0.60;
    config.memory.tool_grounding.persist_success_procedure = false;
    config.memory.tool_grounding.persist_user_visible_episodic = true;
    let affect = AffectState::neutral("ene");
    let tools = vec![ToolResultSummary {
        tool_name: "web".to_string(),
        success: true,
        summary: "Found official docs page".to_string(),
    }];

    let input = PostTurnInput {
        turn: TurnInput {
            user_message: "search docs",
            assistant_message: Some("done"),
            tool_results: &tools,
        },
        affect: &affect,
        character_id: "ene",
        user_id: "user",
        source_turn: None,
        interrupted: false,
        spoken_text: None,
    };

    MemoryWriter::write_memories(&store, &config, &input, MemoryWriteProviders::default())
        .await
        .expect("write memories");

    let rows = store
        .list_typed_memories_by_source_prefix("ene", "tool:", 16)
        .await
        .expect("list source ref");
    assert!(
        rows.iter().any(|m| m.kind == MemoryKind::Episodic),
        "expected episodic memory from tool grounding, got {rows:?}"
    );
}

#[tokio::test]
async fn memory_writer_default_does_not_persist_successful_tools_without_llm() {
    let store = MemoryStore::open_in_memory(4).await.expect("open store");
    let config = MindConfig::default();
    let tools = vec![ToolResultSummary {
        tool_name: "shell.execute".to_string(),
        success: true,
        summary: "ls output here".to_string(),
    }];
    let affect = AffectState::neutral("ene");
    let input = PostTurnInput {
        turn: TurnInput {
            user_message: "list files",
            assistant_message: Some("done"),
            tool_results: &tools,
        },
        affect: &affect,
        character_id: "ene",
        user_id: "user",
        source_turn: None,
        interrupted: false,
        spoken_text: None,
    };

    MemoryWriter::write_memories(&store, &config, &input, MemoryWriteProviders::default())
        .await
        .expect("write memories");

    let rows = store
        .list_typed_memories_by_source_prefix("ene", "tool:", 16)
        .await
        .expect("list");
    assert!(
        rows.is_empty(),
        "default config must not auto-persist successful tools: {rows:?}"
    );
}
