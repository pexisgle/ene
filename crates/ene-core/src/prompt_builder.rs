use ene_memory::{KeyFact, RecalledSummary, format_summaries_for_prompt};
use ene_provider::{LlmMessage, UserMessagePart};
use ene_session::{CharacterCardV3, Role, expand_cbs_macros, resolve_expressions};

fn sys_msg(content: impl Into<String>) -> LlmMessage {
    LlmMessage::System {
        content: content.into(),
    }
}

fn user_msg(content: impl Into<String>) -> LlmMessage {
    LlmMessage::User {
        parts: vec![UserMessagePart::Text {
            text: content.into(),
        }],
    }
}

fn asst_msg(content: impl Into<String>) -> LlmMessage {
    LlmMessage::Assistant {
        content: Some(content.into()),
        tool_calls: None,
    }
}

/// Builds the system prompt from a character card, including runtime rules,
/// personality, scenario, and description.
pub fn build_system_prompt(card: &CharacterCardV3, runtime_rules: &str, user_name: &str) -> String {
    let char_name = card.data.get_character_name();

    let mut parts = Vec::new();

    if !runtime_rules.trim().is_empty() {
        parts.push(runtime_rules.to_string());
    }

    if !card.data.system_prompt.trim().is_empty() {
        parts.push(card.data.system_prompt.clone());
    }

    if !card.data.personality.trim().is_empty() {
        parts.push(format!("Personality:\n{}", card.data.personality));
    }

    if !card.data.scenario.trim().is_empty() {
        parts.push(format!("Scenario:\n{}", card.data.scenario));
    }

    if !card.data.description.trim().is_empty() {
        parts.push(format!("Description:\n{}", card.data.description));
    }

    let combined = parts.join("\n\n");
    expand_cbs_macros(&combined, char_name, user_name)
}

/// Builds the expression protocol instructions (PHI) for emotion token emission.
pub fn build_expression_phi(card: &CharacterCardV3) -> Option<String> {
    let resolved = resolve_expressions(card);

    let auto_phi: Option<String> = if resolved.is_empty() {
        None
    } else {
        let list: Vec<String> = resolved
            .iter()
            .map(|e| {
                if e.description.is_empty() {
                    format!("  - {}", e.name)
                } else {
                    format!("  - {} ({})", e.name, e.description)
                }
            })
            .collect();

        Some(format!(
            "## Emotion Expression Protocol\n\
             Whenever your response has an emotional tone, embed ONE `<|emo:<name>|>` token \
             at a natural position in the reply (e.g. before the sentence it accompanies).\n\
             Example: `<|emo:happy|>`\n\
             Available emotions:\n{}",
            list.join("\n")
        ))
    };

    let manual = card.data.post_history_instructions.trim().to_string();

    match (auto_phi, manual.is_empty()) {
        (Some(auto), true) => Some(auto),
        (Some(auto), false) => Some(format!("{auto}\n\n{manual}")),
        (None, false) => Some(manual),
        (None, true) => None,
    }
}

/// Assembles the full message list for an AI completion request, including system
/// prompt, example messages, memory recalls, key facts, history, expression PHI,
/// and the final user input.
pub fn build_messages(
    card: &CharacterCardV3,
    user_input: &str,
    history: &[(Role, String)],
    runtime_context: &str,
    runtime_rules: &str,
    user_name: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[KeyFact],
) -> Result<Vec<LlmMessage>, String> {
    let mut messages: Vec<LlmMessage> = Vec::new();
    let char_name = card.data.get_character_name();

    let sys_prompt = build_system_prompt(card, runtime_rules, user_name);
    if !sys_prompt.trim().is_empty() {
        messages.push(sys_msg(sys_prompt));
    }

    if history.is_empty() && !card.data.mes_example.trim().is_empty() {
        let ex = expand_cbs_macros(&card.data.mes_example, char_name, user_name);
        messages.push(sys_msg(format!("Example Messages:\n{}", ex)));
    }

    if !recalled_summaries.is_empty() {
        let summary_block = format_summaries_for_prompt(recalled_summaries);
        if !summary_block.trim().is_empty() {
            messages.push(sys_msg(summary_block));
        }
    }

    if !key_facts.is_empty() {
        let facts_block = format!(
            "[Known facts about {user_name}]\n{}",
            key_facts
                .iter()
                .map(|f| format!("• {}: {}", f.key, f.value))
                .collect::<Vec<_>>()
                .join("\n")
        );
        messages.push(sys_msg(facts_block));
    }

    for (role, content) in history {
        match role {
            Role::User => messages.push(user_msg(content.clone())),
            Role::Assistant => messages.push(asst_msg(content.clone())),
            Role::System => messages.push(sys_msg(content.clone())),
        }
    }

    if let Some(phi) = build_expression_phi(card) {
        let phi_expanded = expand_cbs_macros(&phi, char_name, user_name);
        messages.push(sys_msg(phi_expanded));
    }

    let mut final_input = user_input.to_string();
    if !runtime_context.trim().is_empty() {
        final_input.push_str("\n\n[Runtime Context]\n");
        final_input.push_str(runtime_context);
    }

    messages.push(user_msg(final_input));

    Ok(messages)
}
