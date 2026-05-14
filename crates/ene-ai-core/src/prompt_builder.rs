use super::character_card::{expand_cbs_macros, resolve_expressions, CharacterCardV3};
use crate::memory::store::{KeyFact, RecalledSummary};
use crate::memory::recall::format_summaries_for_prompt;
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs, Role,
    ChatCompletionTool, ChatCompletionTools, FunctionObject,
};

pub fn build_tools(tools: &[crate::tools::definition::ToolDefinition]) -> Result<Vec<ChatCompletionTools>, String> {
    let mut res = Vec::new();
    for t in tools {
        let func = FunctionObject {
            name: t.name.clone(),
            description: Some(t.description.clone()),
            parameters: Some(t.parameters.clone()),
            strict: None,
        };
        res.push(ChatCompletionTools::Function(ChatCompletionTool {
            function: func,
        }));
    }
    Ok(res)
}

pub fn build_system_prompt(
    card: &CharacterCardV3,
    runtime_rules: &str,
    user_name: &str,
) -> String {
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
             Whenever your response has an emotional tone, embed ONE `<|ACT:{{...}}|>` token \
             at a natural position in the reply (e.g. before the sentence it accompanies).\n\
             Use JSON format: `<|ACT:{{\"emotion\":\"<name>\"}}|>`\n\
             Available emotions:\n{}",
            list.join("\n")
        ))
    };

    let manual = card.data.post_history_instructions.trim().to_string();

    match (auto_phi, manual.is_empty()) {
        (Some(auto), true)  => Some(auto),
        (Some(auto), false) => Some(format!("{auto}\n\n{manual}")),
        (None, false)       => Some(manual),
        (None, true)        => None,
    }
}

pub fn build_messages(
    card: &CharacterCardV3,
    user_input: &str,
    history: &[(Role, String)],
    runtime_context: &str,
    runtime_rules: &str,
    user_name: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[KeyFact],
) -> Result<Vec<ChatCompletionRequestMessage>, String> {
    let mut messages: Vec<ChatCompletionRequestMessage> = Vec::new();
    let char_name = card.data.get_character_name();
    
    let sys_prompt = build_system_prompt(card, runtime_rules, user_name);
    if !sys_prompt.trim().is_empty() {
        let msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(sys_prompt)
            .build()
            .map_err(|e| format!("Failed to build system message: {e}"))?;
        messages.push(msg.into());
    }

    if history.is_empty() && !card.data.mes_example.trim().is_empty() {
        let ex = expand_cbs_macros(&card.data.mes_example, char_name, user_name);
        let msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(format!("Example Messages:\n{}", ex))
            .build()
            .map_err(|e| format!("Failed to build example message: {e}"))?;
        messages.push(msg.into());
    }

    if !recalled_summaries.is_empty() {
        let summary_block = format_summaries_for_prompt(recalled_summaries);
        if !summary_block.trim().is_empty() {
            let msg = ChatCompletionRequestSystemMessageArgs::default()
                .content(summary_block)
                .build()
                .map_err(|e| format!("Failed to build summary block message: {e}"))?;
            messages.push(msg.into());
        }
    }

    if !key_facts.is_empty() {
        let facts_block = format!(
            "[Known facts about {user_name}]\n{}",
            key_facts.iter().map(|f| format!("• {}: {}", f.key, f.value)).collect::<Vec<_>>().join("\n")
        );
        let msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(facts_block)
            .build()
            .map_err(|e| format!("Failed to build key facts message: {e}"))?;
        messages.push(msg.into());
    }

    for (role, content) in history {
        match role {
            Role::User => {
                let msg = ChatCompletionRequestUserMessageArgs::default()
                    .content(content.clone())
                    .build()
                    .map_err(|e| format!("Failed to build user history msg: {e}"))?;
                messages.push(msg.into());
            }
            Role::Assistant => {
                let msg = ChatCompletionRequestAssistantMessageArgs::default()
                    .content(content.clone())
                    .build()
                    .map_err(|e| format!("Failed to build assistant history msg: {e}"))?;
                messages.push(msg.into());
            }
            Role::System => {
                let msg = ChatCompletionRequestSystemMessageArgs::default()
                    .content(content.clone())
                    .build()
                    .map_err(|e| format!("Failed to build system history msg: {e}"))?;
                messages.push(msg.into());
            }
            _ => {}
        }
    }
    
    if let Some(phi) = build_expression_phi(card) {
        let phi_expanded = expand_cbs_macros(&phi, char_name, user_name);
        let msg = ChatCompletionRequestSystemMessageArgs::default()
            .content(phi_expanded)
            .build()
            .map_err(|e| format!("Failed to build PHI message: {e}"))?;
        messages.push(msg.into());
    }
    
    let mut final_input = user_input.to_string();
    if !runtime_context.trim().is_empty() {
        final_input.push_str("\n\n[Runtime Context]\n");
        final_input.push_str(runtime_context);
    }
    
    let msg = ChatCompletionRequestUserMessageArgs::default()
        .content(final_input)
        .build()
        .map_err(|e| format!("Failed to build current user msg: {e}"))?;
    messages.push(msg.into());
    
    Ok(messages)
}
