use ene_ai::{LlmMessage, Role, UserMessagePart};
use ene_config::{CharacterCardV3, PromptLibrary, expand_cbs_macros, resolve_expressions};

/// Input parameters for [`build_messages`].
#[derive(Debug, Clone)]
pub struct MessageBuildContext<'a> {
    /// The loaded character card.
    pub card: &'a CharacterCardV3,
    /// The user's current input text.
    pub user_input: &'a str,
    /// Conversation history entries.
    pub history: &'a [ene_mind::HistoryEntry],
    /// Optional runtime context appended to the user input (`None` is treated as empty).
    pub runtime_context: Option<&'a str>,
    /// Runtime rules prepended to the system prompt.
    pub runtime_rules: &'a str,
    /// Display name of the user.
    pub user_name: &'a str,
    /// Prompt template library (caller provides; defaults to built-in English).
    pub prompts: &'a PromptLibrary,
    /// Whether emotion processing is enabled (selects marker PHI vs natural-dialogue contract).
    pub emotion_enabled: bool,
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
pub fn build_system_prompt(
    card: &CharacterCardV3,
    runtime_rules: &str,
    user_name: &str,
    prompts: &PromptLibrary,
) -> String {
    let runtime_rules = if runtime_rules.trim().is_empty() {
        ene_config::DEFAULT_RUNTIME_RULES
    } else {
        runtime_rules
    };
    let char_name = card.data.get_character_name();
    let mut parts: Vec<String> = Vec::new();

    // 1. Desktop mascot context frame — always first so the model
    //    immediately understands the overlay / short-response constraint.
    let mascot_context = prompts.system().render_mascot_context(char_name, user_name);
    if !mascot_context.is_empty() {
        parts.push(mascot_context);
    }

    // 2. Runtime rules (user-configurable behaviour overrides).
    if !runtime_rules.trim().is_empty() {
        let header = &prompts.system().behavior_rules_header;
        if header.is_empty() {
            parts.push(runtime_rules.to_string());
        } else {
            parts.push(format!("{header}\n{runtime_rules}"));
        }
    }

    // 3. Character identity block from the card.
    let char_header = &prompts.system().character_header;
    let mut char_parts: Vec<String> = Vec::new();

    if !card.data.system_prompt.trim().is_empty() {
        char_parts.push(card.data.system_prompt.clone());
    }
    if !card.data.personality.trim().is_empty() {
        let h = &prompts.system().personality_header;
        char_parts.push(format!("{h}\n{}", card.data.personality));
    }
    if !card.data.description.trim().is_empty() {
        let h = &prompts.system().background_header;
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
        let h = &prompts.system().scene_header;
        if h.is_empty() {
            parts.push(card.data.scenario.clone());
        } else {
            parts.push(format!("{h}\n{}", card.data.scenario));
        }
    }

    let combined = parts.join("\n\n");
    expand_cbs_macros(&combined, char_name, user_name)
}

/// Builds the Performance Output Protocol (PHI) block.
///
/// Produces a concise, command-tone instruction with concrete examples
/// so that even lower-capability models reliably output the `<|perf:expr=NAME|>`
/// token in the right position.
pub fn build_expression_phi(card: &CharacterCardV3, prompts: &PromptLibrary) -> Option<String> {
    let char_name = card.data.get_character_name();
    let resolved = resolve_expressions(card);

    let auto_phi: Option<String> = if resolved.is_empty() {
        None
    } else {
        let list: Vec<String> = resolved
            .iter()
            .map(|e| {
                if e.description.is_empty() {
                    format!("- `<|perf:expr={}|>`", e.name)
                } else {
                    format!("- `<|perf:expr={}|>` — {}", e.name, e.description)
                }
            })
            .collect();

        let header = &prompts.emotion().header;
        let rule = &prompts.emotion().rule;
        let token_header = &prompts.emotion().token_header;
        let examples_header = &prompts.emotion().examples_header;

        let examples = [
            &prompts.emotion().example_happy,
            &prompts.emotion().example_sad,
            &prompts.emotion().example_angry,
            &prompts.emotion().example_neutral,
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

/// Builds the natural-dialogue output contract for engine-managed expression (#91).
///
/// Instructs the LLM to respond in plain dialogue without inline performance markers.
/// Expression is resolved by the cognitive runtime Output Arbiter after the turn.
pub fn build_natural_dialogue_contract(
    card: &CharacterCardV3,
    prompts: &PromptLibrary,
    user_name: &str,
) -> Option<String> {
    let char_name = card.data.get_character_name();
    let manual = card.data.post_history_instructions.trim();

    let mut contract = expand_cbs_macros(
        &prompts.emotion().natural_dialogue_contract,
        char_name,
        user_name,
    );

    if !manual.is_empty() {
        contract.push_str("\n\n");
        contract.push_str(&expand_cbs_macros(manual, char_name, user_name));
    }
    Some(contract)
}

/// Selects the post-history output block for the cognitive streaming path.
pub fn build_cognitive_output_contract(
    card: &CharacterCardV3,
    prompts: &PromptLibrary,
    emotion_enabled: bool,
    user_name: &str,
) -> Option<String> {
    if emotion_enabled {
        build_natural_dialogue_contract(card, prompts, user_name)
    } else {
        build_expression_phi(card, prompts)
    }
}

/// Assembles the full message list for an AI completion request.
///
/// Message order:
/// 1. `System` — mascot-aware system prompt (rules + character identity + scene)
/// 2. `System` — example messages (first turn only)
/// 3. History — alternating `User` / `Assistant` turns
/// 4. `System` — Post-history output contract (marker PHI or natural-dialogue, based on `emotion_enabled`)
/// 5. `User`   — current user input (+ optional runtime context)
///
/// Legacy recalled summaries / keyfacts are no longer injected (#125).
pub fn build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<LlmMessage>, crate::error::EneRuntimeError> {
    let mut messages: Vec<LlmMessage> = Vec::new();
    let char_name = ctx.card.data.get_character_name();

    let sys_prompt = build_system_prompt(ctx.card, ctx.runtime_rules, ctx.user_name, ctx.prompts);
    if !sys_prompt.trim().is_empty() {
        messages.push(sys_msg(sys_prompt));
    }

    if ctx.history.is_empty() && !ctx.card.data.mes_example.trim().is_empty() {
        let ex = expand_cbs_macros(&ctx.card.data.mes_example, char_name, ctx.user_name);
        let header = &ctx.prompts.system().examples_header;
        messages.push(sys_msg(format!("{header}\n{ex}")));
    }

    for entry in ctx.history {
        match entry.role {
            Role::User => messages.push(user_msg(entry.content.clone())),
            Role::Assistant => messages.push(asst_msg(entry.content.clone())),
            Role::System => messages.push(sys_msg(entry.content.clone())),
        }
    }

    if let Some(phi) =
        build_cognitive_output_contract(ctx.card, ctx.prompts, ctx.emotion_enabled, ctx.user_name)
    {
        let phi_expanded = expand_cbs_macros(&phi, char_name, ctx.user_name);
        messages.push(sys_msg(phi_expanded));
    }

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
            phi.contains("<|perf:expr=happy|>"),
            "phi should list happy token: {phi}"
        );
        assert!(
            phi.contains("Output contract"),
            "phi should contain output contract directive: {phi}"
        );
    }
}
