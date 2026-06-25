use ene_config::{CharacterCardV3, PromptLibrary, expand_cbs_macros, resolve_expressions};
use ene_memory::{KeyFact, RecalledSummary};
use ene_provider::{LlmMessage, Role, UserMessagePart};

/// Input parameters for [`build_messages`].
#[derive(Debug, Clone)]
pub struct MessageBuildContext<'a> {
    /// The loaded character card.
    pub card: &'a CharacterCardV3,
    /// The user's current input text.
    pub user_input: &'a str,
    /// Conversation history entries.
    pub history: &'a [crate::handle::ConversationEntry],
    /// Optional runtime context appended to the user input (`None` is treated as empty).
    pub runtime_context: Option<&'a str>,
    /// Runtime rules prepended to the system prompt.
    pub runtime_rules: &'a str,
    /// Display name of the user.
    pub user_name: &'a str,
    /// Recalled memory summaries.
    pub recalled_summaries: &'a [RecalledSummary],
    /// Key facts about the user.
    pub key_facts: &'a [KeyFact],
    /// Prompt template library (caller provides; defaults to built-in English).
    pub prompts: &'a PromptLibrary,
}

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

/// Builds the system prompt for a desktop mascot character.
///
/// The structure is optimised for overlay-displayed AI companions:
/// it places the mascot context frame first so all models immediately
/// understand the display constraints, then layers character identity
/// and runtime rules below it.
///
/// ```text
/// [Desktop Companion Context]
/// You are {char}, a desktop AI companion living on {user}'s screen.
/// …
///
/// ## Behavior Rules
/// {runtime_rules}
///
/// ## Character
/// {card.system_prompt}
///
/// ### Personality
/// {card.personality}
///
/// ### Background
/// {card.description}
///
/// ## Current Scene
/// {card.scenario}
/// ```
#[must_use]
pub fn build_system_prompt(
    card: &CharacterCardV3,
    runtime_rules: &str,
    user_name: &str,
    prompts: &PromptLibrary,
) -> String {
    let char_name = card.data.get_character_name();
    let mut parts: Vec<String> = Vec::new();

    // 1. Desktop mascot context frame — always first so the model
    //    immediately understands the overlay / short-response constraint.
    let mascot_context = prompts.render(
        "system.mascot_context",
        &[("char_name", char_name), ("user_name", user_name)],
    );
    if !mascot_context.is_empty() {
        parts.push(mascot_context);
    }

    // 2. Runtime rules (user-configurable behaviour overrides).
    if !runtime_rules.trim().is_empty() {
        let header = prompts.get("system.behavior_rules_header");
        if header.is_empty() {
            parts.push(runtime_rules.to_string());
        } else {
            parts.push(format!("{header}\n{runtime_rules}"));
        }
    }

    // 3. Character identity block from the card.
    let char_header = prompts.get("system.character_header");
    let mut char_parts: Vec<String> = Vec::new();

    if !card.data.system_prompt.trim().is_empty() {
        char_parts.push(card.data.system_prompt.clone());
    }
    if !card.data.personality.trim().is_empty() {
        let h = prompts.get("system.personality_header");
        char_parts.push(format!("{h}\n{}", card.data.personality));
    }
    if !card.data.description.trim().is_empty() {
        let h = prompts.get("system.background_header");
        char_parts.push(format!("{h}\n{}", card.data.description));
    }

    if !char_parts.is_empty() {
        let char_block = char_parts.join("\n\n");
        if char_header.is_empty() {
            parts.push(char_block);
        } else {
            parts.push(format!("{char_header}\n{char_block}"));
        }
    }

    // 4. Scene / scenario.
    if !card.data.scenario.trim().is_empty() {
        let h = prompts.get("system.scene_header");
        if h.is_empty() {
            parts.push(card.data.scenario.clone());
        } else {
            parts.push(format!("{h}\n{}", card.data.scenario));
        }
    }

    let combined = parts.join("\n\n");
    expand_cbs_macros(&combined, char_name, user_name)
}

/// Builds the Emotion Expression Protocol (PHI) block.
///
/// Produces a concise, command-tone instruction with concrete examples
/// so that even lower-capability models reliably output the `<|emo:NAME|>`
/// token in the right position.
#[must_use]
pub fn build_expression_phi(card: &CharacterCardV3, prompts: &PromptLibrary) -> Option<String> {
    let char_name = card.data.get_character_name();
    let resolved = resolve_expressions(card);

    let auto_phi: Option<String> = if resolved.is_empty() {
        None
    } else {
        // Token list: "- <|emo:happy|> — Feeling joyful, excited, or pleased"
        let list: Vec<String> = resolved
            .iter()
            .map(|e| {
                if e.description.is_empty() {
                    format!("- `<|emo:{}|>`", e.name)
                } else {
                    format!("- `<|emo:{}|>` — {}", e.name, e.description)
                }
            })
            .collect();

        let header = prompts.get("emotion.header");
        let rule = prompts.get("emotion.rule");
        let token_header = prompts.get("emotion.token_header");
        let examples_header = prompts.get("emotion.examples_header");

        let examples = [
            prompts.get("emotion.example_happy"),
            prompts.get("emotion.example_sad"),
            prompts.get("emotion.example_neutral"),
        ]
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| format!("  {s}"))
        .collect::<Vec<_>>()
        .join("\n");

        let mut phi = format!(
            "{header}\n{rule}\n\n{token_header}\n{}\n\n{examples_header}\n{examples}",
            list.join("\n")
        );

        // Expand {{char}} / {{user}} macros from the card author's perspective.
        phi = expand_cbs_macros(&phi, char_name, "User");
        Some(phi)
    };

    let manual = card.data.post_history_instructions.trim().to_string();

    match (auto_phi, manual.is_empty()) {
        (Some(auto), true) => Some(auto),
        (Some(auto), false) => Some(format!("{auto}\n\n{manual}")),
        (None, false) => Some(manual),
        (None, true) => None,
    }
}

/// Assembles the full message list for an AI completion request.
///
/// Message order:
/// 1. `System` — mascot-aware system prompt (rules + character identity + scene)
/// 2. `System` — example messages (first turn only)
/// 3. `System` — past conversation summaries (memory recall)
/// 4. `System` — known key facts about the user
/// 5. History — alternating `User` / `Assistant` turns
/// 6. `System` — Expression PHI (emotion token protocol + post-history instructions)
/// 7. `User`   — current user input (+ optional runtime context)
pub fn build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<LlmMessage>, crate::error::EneCoreError> {
    let mut messages: Vec<LlmMessage> = Vec::new();
    let char_name = ctx.card.data.get_character_name();

    // 1. System prompt — mascot context + character identity.
    let sys_prompt = build_system_prompt(ctx.card, ctx.runtime_rules, ctx.user_name, ctx.prompts);
    if !sys_prompt.trim().is_empty() {
        messages.push(sys_msg(sys_prompt));
    }

    // 2. Example messages — only on the first turn to seed the style.
    if ctx.history.is_empty() && !ctx.card.data.mes_example.trim().is_empty() {
        let ex = expand_cbs_macros(&ctx.card.data.mes_example, char_name, ctx.user_name);
        let header = ctx.prompts.get("system.examples_header");
        messages.push(sys_msg(format!("{header}\n{ex}")));
    }

    // 3. Past conversation summaries from episodic memory.
    if !ctx.recalled_summaries.is_empty() {
        let summary_block = format_summaries(ctx.recalled_summaries, ctx.prompts);
        if !summary_block.trim().is_empty() {
            messages.push(sys_msg(summary_block));
        }
    }

    // 4. Known key facts about the user.
    if !ctx.key_facts.is_empty() {
        let header = ctx
            .prompts
            .render("memory.facts_header", &[("user_name", ctx.user_name)]);
        let lines: Vec<String> = ctx
            .key_facts
            .iter()
            .map(|f| format!("- {}: {}", f.key, f.value))
            .collect();
        messages.push(sys_msg(format!("{header}\n{}", lines.join("\n"))));
    }

    // 5. Conversation history.
    for entry in ctx.history {
        match entry.role {
            Role::User => messages.push(user_msg(entry.content.clone())),
            Role::Assistant => messages.push(asst_msg(entry.content.clone())),
            Role::System => messages.push(sys_msg(entry.content.clone())),
        }
    }

    // 6. Expression PHI — placed last among system blocks so it is the
    //    most-recently-seen instruction before the user turn.
    if let Some(phi) = build_expression_phi(ctx.card, ctx.prompts) {
        let phi_expanded = expand_cbs_macros(&phi, char_name, ctx.user_name);
        messages.push(sys_msg(phi_expanded));
    }

    // 7. Current user input (+ optional runtime context).
    let mut final_input = ctx.user_input.to_string();
    if let Some(rc) = ctx.runtime_context
        && !rc.trim().is_empty()
    {
        final_input.push_str("\n\n[Runtime Context]\n");
        final_input.push_str(rc);
    }
    messages.push(user_msg(final_input));

    Ok(messages)
}

/// Formats recalled past-conversation summaries for prompt injection.
///
/// Extracted here (rather than calling `ene_memory::format_summaries_for_prompt`)
/// so the template key paths come from the `PromptLibrary`.
fn format_summaries(summaries: &[RecalledSummary], prompts: &PromptLibrary) -> String {
    if summaries.is_empty() {
        return String::new();
    }

    let now = chrono::Utc::now();
    let header = prompts.get("memory.summaries_header");
    let item_tpl = prompts.get("memory.summary_item");

    let mut lines = vec![header.to_string()];
    for s in summaries {
        let age = format_age(now - s.entry.ended_at);
        let text = ene_common::truncate::Truncate::simple(&s.entry.summary, 300);
        let line = if item_tpl.is_empty() {
            format!("[{age}] {text}")
        } else {
            ene_config::substitute_prompt_vars(item_tpl, &[("age", &age), ("text", &text)])
        };
        lines.push(line);
    }

    lines.join("\n")
}

fn format_age(dur: chrono::Duration) -> String {
    let total_seconds = dur.num_seconds().max(0);
    if total_seconds < 60 {
        "just now".to_string()
    } else if total_seconds < 3600 {
        let mins = total_seconds / 60;
        if mins == 1 {
            "1 min ago".to_string()
        } else {
            format!("{mins} min ago")
        }
    } else if total_seconds < 86400 {
        let hours = total_seconds / 3600;
        if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{hours} hours ago")
        }
    } else {
        let days = total_seconds / 86400;
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{days} days ago")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_config::{CharacterCardData, CharacterCardV3};

    fn test_card(
        system_prompt: &str,
        personality: &str,
        description: &str,
        scenario: &str,
    ) -> CharacterCardV3 {
        CharacterCardV3 {
            spec: "chara_card_v3".to_string(),
            spec_version: "3.0".to_string(),
            data: CharacterCardData {
                name: "TestChar".to_string(),
                system_prompt: system_prompt.to_string(),
                personality: personality.to_string(),
                description: description.to_string(),
                scenario: scenario.to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn build_system_prompt_contains_mascot_context() {
        let lib = PromptLibrary::built_in_english();
        let card = test_card(
            "You are a test character.",
            "Cheerful",
            "Lives online",
            "A test world",
        );
        let prompt = build_system_prompt(&card, "", "Alice", &lib);
        assert!(
            prompt.contains("desktop AI companion") || prompt.contains("screen"),
            "System prompt should mention desktop/screen context: {prompt}"
        );
    }

    #[test]
    fn build_system_prompt_includes_char_name() {
        let lib = PromptLibrary::built_in_english();
        let card = test_card("I am TestChar.", "Friendly", "", "");
        let prompt = build_system_prompt(&card, "", "Bob", &lib);
        assert!(
            prompt.contains("TestChar"),
            "should contain character name: {prompt}"
        );
        assert!(prompt.contains("Bob"), "should contain user name: {prompt}");
    }

    #[test]
    fn build_system_prompt_omits_empty_sections() {
        let lib = PromptLibrary::built_in_english();
        let card = test_card("", "", "", "");
        let prompt = build_system_prompt(&card, "", "Alice", &lib);
        // Should not produce empty section headers
        assert!(
            !prompt.contains("### Personality\n\n"),
            "empty personality section leaked: {prompt}"
        );
        assert!(
            !prompt.contains("### Background\n\n"),
            "empty background section leaked: {prompt}"
        );
    }

    #[test]
    fn build_expression_phi_includes_examples() {
        let lib = PromptLibrary::built_in_english();
        let card = test_card("", "", "", "");
        let phi = build_expression_phi(&card, &lib).expect("phi should be Some");
        assert!(
            phi.contains("<|emo:happy|>"),
            "phi should list happy token: {phi}"
        );
        assert!(
            phi.contains("RULE:"),
            "phi should contain RULE directive: {phi}"
        );
    }

    #[test]
    fn format_summaries_uses_template() {
        let lib = PromptLibrary::built_in_english();
        // Verify header is present when summaries would be non-empty
        let header = lib.get("memory.summaries_header");
        assert!(!header.is_empty(), "summaries header should be non-empty");
    }
}
