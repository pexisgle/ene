//! Tool-result grounding helpers for Phase 8 (#92).

use crate::config::ToolGroundingConfig;
use crate::memory_writer::candidate::{MemoryCandidate, ToolResultSummary};
use ene_store::typed_memory::MemoryKind;

const SCREENSHOT_SENTINEL: &str = "[Screenshot successfully captured and sent to vision system]";

/// Build a bounded, memory-safe summary for one tool result.
pub fn summarize_tool_result(
    tool_name: &str,
    raw_output: &str,
    success: bool,
    max_summary_chars: usize,
) -> ToolResultSummary {
    let mut summary = normalize_tool_output(raw_output);
    if max_summary_chars > 0 && summary.chars().count() > max_summary_chars {
        let mut truncated: String = summary.chars().take(max_summary_chars).collect();
        truncated.push_str("...");
        summary = truncated;
    }
    ToolResultSummary {
        tool_name: tool_name.to_string(),
        success,
        summary,
    }
}

/// Classify tool summaries into memory candidates with guardrails.
pub fn extract_tool_candidates(
    tool_results: &[ToolResultSummary],
    cfg: &ToolGroundingConfig,
) -> Vec<MemoryCandidate> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut out = Vec::new();
    for tr in tool_results {
        let summary = tr.summary.trim();
        if summary.is_empty() {
            continue;
        }
        let summary_lower = summary.to_lowercase();

        if tr.success && cfg.persist_success_procedure {
            out.push(MemoryCandidate {
                kind: MemoryKind::Procedure,
                title: format!("tool:{}", tr.tool_name),
                content: format!("[{}] {}", tr.tool_name, summary),
                source_quote: String::new(),
                confidence: 0.70,
                should_persist: true,
                deletion_target_key: None,
                commitment_due: None,
                tags: Vec::new(),
            });
        }

        if !tr.success && cfg.persist_failure_reflection {
            out.push(MemoryCandidate {
                kind: MemoryKind::Reflection,
                title: format!("tool failure:{}", tr.tool_name),
                content: format!(
                    "Tool '{}' failed. Avoid repeating the same failed path without new evidence. {}",
                    tr.tool_name, summary
                ),
                source_quote: String::new(),
                confidence: 0.65,
                should_persist: true,
                deletion_target_key: None,
                commitment_due: None,
                tags: Vec::new(),
            });
        }

        if tr.success
            && cfg.persist_user_visible_episodic
            && summary.chars().count() <= 200
            && summary != SCREENSHOT_SENTINEL
            && !summary_lower.starts_with("error")
        {
            out.push(MemoryCandidate {
                kind: MemoryKind::Episodic,
                title: format!("tool outcome:{}", tr.tool_name),
                content: format!("Tool '{}' outcome: {}", tr.tool_name, summary),
                source_quote: String::new(),
                confidence: 0.60,
                should_persist: true,
                deletion_target_key: None,
                commitment_due: None,
                tags: Vec::new(),
            });
        }
    }
    out
}

fn normalize_tool_output(raw_output: &str) -> String {
    if is_screenshot_payload(raw_output) {
        return SCREENSHOT_SENTINEL.to_string();
    }

    let output = raw_output.trim();
    if output.is_empty() {
        return "tool returned empty output".to_string();
    }
    if let Some(rest) = output.strip_prefix("Error executing tool:") {
        return format!("error: {}", rest.trim());
    }
    output.to_string()
}

fn is_screenshot_payload(result: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|json| {
            let is_screenshot = json.get("type").and_then(|v| v.as_str()) == Some("screenshot");
            let has_data = json.get("data").and_then(|v| v.as_str()).is_some();
            is_screenshot.then_some(has_data)
        })
        .unwrap_or(false)
}
