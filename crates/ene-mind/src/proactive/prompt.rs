//! Decision prompt for proactive speech (JSON only; no utterance body).

use crate::proactive::ProactiveContext;
use ene_ai::{LlmMessage, Role, UserMessagePart};
use ene_config::PromptLibrary;

/// Build chat messages that instruct the model to return decision JSON only.
#[must_use]
pub fn build_decision_messages(context: &ProactiveContext) -> Vec<LlmMessage> {
    // Backend LLM instructions stay English (AGENTS.md); JA templates exist for locale parity.
    let prompts = PromptLibrary::load("en");
    let system = LlmMessage::System {
        content: prompts.proactive().decision_system.trim().to_string(),
    };
    let user = LlmMessage::User {
        parts: vec![UserMessagePart::Text {
            text: format_context_block(context),
        }],
    };
    vec![system, user]
}

fn format_context_block(context: &ProactiveContext) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "seconds_since_user_input: {}",
        context.seconds_since_user_input
    ));
    lines.push(format!(
        "proactive_turns_this_session: {}",
        context.suppression.proactive_turns_this_session
    ));

    if let Some(affect) = &context.affect_summary {
        lines.push(format!("affect: {affect}"));
    }

    if let Some(activity) = &context.activity {
        let idle = activity
            .idle_seconds
            .map_or_else(|| "unknown".to_string(), |s| s.to_string());
        lines.push(format!(
            "activity: idle_seconds={idle} window=\"{}\" change=\"{}\"",
            activity.active_window_label, activity.recent_change
        ));
    }

    if let Some(screen) = &context.screen_summary {
        lines.push(format!("screen_summary: {screen}"));
    }

    if !context.commitments.is_empty() {
        lines.push("commitments:".to_string());
        for c in &context.commitments {
            lines.push(format!("- {c}"));
        }
    }

    if !context.history.is_empty() {
        lines.push("recent_conversation:".to_string());
        for entry in &context.history {
            let role = match entry.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            };
            lines.push(format!("{role}: {}", entry.content));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::HistoryEntry;
    use crate::proactive::{ActivitySnapshot, ProactiveSuppressionState};

    #[test]
    fn omits_disabled_sections() {
        let ctx = ProactiveContext {
            history: vec![HistoryEntry {
                role: Role::User,
                content: "hello".into(),
            }],
            seconds_since_user_input: 90,
            activity: None,
            screen_summary: None,
            affect_summary: None,
            commitments: vec![],
            suppression: ProactiveSuppressionState::default(),
        };
        let text = format_context_block(&ctx);
        assert!(text.contains("recent_conversation:"));
        assert!(!text.contains("activity:"));
        assert!(!text.contains("screen_summary:"));
    }

    #[test]
    fn includes_activity_when_present() {
        let ctx = ProactiveContext {
            history: vec![],
            seconds_since_user_input: 90,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(90),
                active_window_label: "Code".into(),
                recent_change: "focus".into(),
            }),
            screen_summary: Some("editor open".into()),
            affect_summary: Some("valence=0.10".into()),
            commitments: vec!["reply later".into()],
            suppression: ProactiveSuppressionState::default(),
        };
        let text = format_context_block(&ctx);
        assert!(text.contains("activity:"));
        assert!(text.contains("screen_summary:"));
        assert!(text.contains("commitments:"));
        assert!(text.contains("affect:"));
    }
}
