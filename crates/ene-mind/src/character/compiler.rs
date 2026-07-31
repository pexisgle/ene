//! Deterministic `CCv3` → Identity Kernel compilation (#82).

use ene_config::{CharacterCardV3, MacroContext, UserPersona, expand_cbs_macros_ctx};

use super::kernel::IdentityKernel;

/// Default maximum kernel size in approximate tokens (4 chars/token heuristic).
pub const DEFAULT_IDENTITY_KERNEL_MAX_TOKENS: usize = 400;

/// Compiles `CCv3` character fields into a compact Identity Kernel.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharacterCompiler;

impl CharacterCompiler {
    /// Compile an Identity Kernel from a V3 character card.
    ///
    /// `user_persona`, when provided, expands the `{{user_persona}}` CBS macro
    /// in card-derived fields (#H-3).
    ///
    /// `pick_seed`, when provided, makes `{{pick}}` resolve to a stable choice
    /// for the lifetime of the chat instead of re-rolling on every per-turn
    /// recompilation (#343). Derive it with
    /// [`ene_config::session_pick_seed`] from a session-scoped key.
    #[must_use]
    pub fn compile(
        card: &CharacterCardV3,
        user_name: &str,
        user_persona: Option<&UserPersona>,
        pick_seed: Option<u64>,
        max_tokens: usize,
    ) -> IdentityKernel {
        let data = &card.data;
        let char_name = data.get_character_name();
        let max_chars = max_tokens.saturating_mul(4).max(256);

        // One shared context so every field expands identically; `{{pick}}`
        // therefore yields the same option in the personality, background, and
        // scene sections of the same kernel (#343).
        let ctx = MacroContext {
            char_name,
            user_name,
            user_persona,
            card: Some(card),
            pick_seed,
            ..MacroContext::default()
        };

        let post_history = optional_expanded(&data.post_history_instructions, ctx).map(|phi| {
            // Expand `{user_name}` at compile time (like every other field) so
            // no literal `{{user}}` placeholder leaks to the LLM (#H-2).
            let reinforcement = format!(
                "\n\nImportant: You are {char_name}. {user_name} is a separate person. \
                 Do not put words in {user_name}'s mouth, do not describe {user_name}'s actions, \
                 and do not speak for {user_name}. If the conversation requires {user_name}'s input, \
                 wait for it."
            );
            format!("{phi}{reinforcement}")
        });

        let core_personality = if !data.personality.trim().is_empty() {
            expand_field(&data.personality, ctx)
        } else if !data.description.trim().is_empty() {
            truncate_chars(&expand_field(&data.description, ctx), 240)
        } else {
            String::from("helpful and consistent")
        };

        let speech_style = derive_speech_style(&data.system_prompt, ctx);

        let anti_impersonation = format!(
            "\n- NEVER speak, act, think, or write for {user_name}. {user_name} controls their own actions.\n\
             - When {user_name} asks you to do something, you may describe YOUR response, not theirs.\n\
             - If {user_name} says something, respond as {char_name}, not as {user_name}."
        );
        let core_lines = [
            "[Identity Kernel]".to_string(),
            format!("Name: {char_name}"),
            format!("Role: desktop companion living on {user_name}'s screen"),
            format!("Core personality: {core_personality}"),
            format!("Speech style: {speech_style}"),
            format!(
                "Hard instruction: remain {char_name} even in long conversations{anti_impersonation}"
            ),
        ];
        let core_block = core_lines.join("\n");

        let mut optional_sections: Vec<String> = Vec::new();

        if !data.system_prompt.trim().is_empty() {
            optional_sections.push(format!(
                "## Core Instructions\n{}",
                expand_field(&data.system_prompt, ctx)
            ));
        }
        if !data.personality.trim().is_empty() && !data.description.trim().is_empty() {
            optional_sections.push(format!(
                "## Background\n{}",
                expand_field(&data.description, ctx)
            ));
        }
        if !data.scenario.trim().is_empty() {
            optional_sections.push(format!(
                "## Current Scene\n{}",
                expand_field(&data.scenario, ctx)
            ));
        }
        if !data.creator_notes.trim().is_empty() {
            optional_sections.push(format!(
                "## Creator Notes\n{}",
                expand_field(&data.creator_notes, ctx)
            ));
        }

        let mut text = core_block.clone();
        for section in &optional_sections {
            let candidate = if text.is_empty() {
                section.clone()
            } else {
                format!("{text}\n\n{section}")
            };
            if candidate.chars().count() <= max_chars {
                text = candidate;
            } else {
                break;
            }
        }

        if text.chars().count() > max_chars {
            text = truncate_preserving_core(&core_block, max_chars);
        }

        IdentityKernel {
            name: char_name.to_string(),
            text,
            post_history_instructions: post_history,
        }
    }
}

fn expand_field(raw: &str, ctx: MacroContext<'_>) -> String {
    expand_cbs_macros_ctx(raw.trim(), &ctx)
}

fn optional_expanded(raw: &str, ctx: MacroContext<'_>) -> Option<String> {
    if raw.trim().is_empty() {
        None
    } else {
        Some(expand_field(raw, ctx))
    }
}

fn derive_speech_style(system_prompt: &str, ctx: MacroContext<'_>) -> String {
    // The speech-style probe intentionally ignores `{{user_persona}}` and the
    // card-field macros (it only sniffs for length keywords), so it reuses the
    // context with those cleared to avoid pulling persona text into the probe.
    let probe_ctx = MacroContext {
        user_persona: None,
        card: None,
        ..ctx
    };
    let expanded = expand_field(system_prompt, probe_ctx);
    let lower = expanded.to_lowercase();
    if lower.contains("short")
        || lower.contains("brief")
        || lower.contains("overlay")
        || lower.contains("concise")
    {
        return String::from("short, warm, suitable for overlay display");
    }
    if expanded.trim().is_empty() {
        return String::from("natural, warm, suitable for overlay display");
    }
    truncate_chars(&expanded, 160)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn truncate_preserving_core(core_block: &str, max_chars: usize) -> String {
    if core_block.chars().count() <= max_chars {
        return core_block.to_string();
    }
    let hard_marker = "Hard instruction:";
    if let Some(idx) = core_block.find(hard_marker) {
        let tail = &core_block[idx..];
        let head_budget = max_chars.saturating_sub(tail.chars().count());
        if head_budget == 0 {
            return truncate_chars(tail, max_chars);
        }
        let head = core_block.chars().take(head_budget).collect::<String>();
        return format!("{head}{tail}");
    }
    truncate_chars(core_block, max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_config::CharacterCardV3;
    use std::fs;
    use std::path::PathBuf;

    fn alicia_card() -> CharacterCardV3 {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/characters/Alicia/character.json");
        let raw = fs::read_to_string(path).expect("read Alicia card");
        serde_json::from_str(&raw).expect("parse Alicia card")
    }

    #[test]
    fn kernel_includes_structured_header_and_hard_instruction() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "Keep responses short for overlay.".into();
        card.data.personality = "Energetic.".into();

        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400);
        assert!(kernel.text.contains("Name: Ene"));
        assert!(kernel.text.contains("Core personality: Energetic."));
        assert!(kernel.text.contains("Speech style:"));
        assert!(kernel.text.contains("Hard instruction: remain Ene"));
        assert!(kernel.text.contains("Keep responses short"));
    }

    #[test]
    fn nickname_preferred_over_name() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Official".into();
        card.data.nickname = "Ene".into();
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400);
        assert!(kernel.text.contains("Name: Ene"));
        assert!(!kernel.text.contains("Official"));
    }

    #[test]
    fn macros_expanded_in_kernel() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Friends with {{user}}.".into();
        let kernel = CharacterCompiler::compile(&card, "Alice", None, None, 400);
        assert!(kernel.text.contains("Friends with Alice."));
        // The anti-impersonation guard is expanded at compile time (#H-2), so no
        // literal `{{user}}` placeholder may leak into the kernel sent to the LLM.
        assert!(
            !kernel.text.contains("{{user}}"),
            "kernel must not leak a literal {{{{user}}}} placeholder: {}",
            kernel.text
        );
        assert!(
            kernel
                .text
                .contains("NEVER speak, act, think, or write for Alice.")
        );
    }

    #[test]
    fn user_persona_macro_expanded_in_kernel() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Knows {{user_persona}} well.".into();
        let persona = ene_config::UserPersona {
            name: "Alice".into(),
            description: Some("A software engineer.".into()),
            relationship: None,
            pronouns: None,
            notes: None,
        };
        let kernel = CharacterCompiler::compile(&card, "Alice", Some(&persona), None, 400);
        assert!(
            kernel.text.contains("Name: Alice"),
            "persona block should be expanded into the kernel: {}",
            kernel.text
        );
        assert!(kernel.text.contains("Description: A software engineer."));
        assert!(
            !kernel.text.contains("{{user_persona}}"),
            "kernel must not leak a literal {{{{user_persona}}}} placeholder: {}",
            kernel.text
        );
    }

    #[test]
    fn post_history_instructions_stored_separately() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.post_history_instructions = "Stay in character.".into();
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400);
        assert!(!kernel.text.contains("Stay in character."));
        let phi = kernel
            .post_history_instructions
            .expect("post_history present");
        assert!(phi.contains("Stay in character."));
        // Anti-impersonation reinforcement is appended and expanded at compile time (#H-2)
        assert!(
            phi.contains("Important: You are Ene"),
            "phi should contain 'Important: You are Ene' but was: {phi}"
        );
        assert!(phi.contains("Do not put words in User's mouth"));
        assert!(
            !phi.contains("{{user}}"),
            "post-history must not leak a literal {{{{user}}}} placeholder: {phi}"
        );
    }

    /// Regression for #343: the identity kernel is recompiled on every turn,
    /// so a seeded `{{pick}}` must resolve to the same trait each time instead
    /// of re-rolling (which previously changed hair colour / hometown per turn).
    #[test]
    fn pick_is_stable_across_recompilations_with_seed() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Hair color: {{pick:red,blue,green,gold,silver}}.".into();

        let seed = Some(ene_config::session_pick_seed("ene:session-1"));
        let first = CharacterCompiler::compile(&card, "User", None, seed, 400);
        for _ in 0..16 {
            let again = CharacterCompiler::compile(&card, "User", None, seed, 400);
            assert_eq!(
                again.text, first.text,
                "seeded {{{{pick}}}} must be stable across recompilations"
            );
        }
    }

    #[test]
    fn alicia_default_card_compiles() {
        let card = alicia_card();
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400);
        assert!(kernel.text.contains("Name: Alicia"));
        assert!(kernel.text.contains("cheerful") || kernel.text.contains("Friendly"));
        assert!(kernel.text.contains("Hard instruction: remain Alicia"));
    }

    #[test]
    fn truncate_drops_optional_sections_before_core_header() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "x".repeat(2_000);
        card.data.description = "y".repeat(2_000);
        // Use enough budget for the core block (which now includes anti-impersonation guard)
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 120);
        assert!(kernel.text.contains("[Identity Kernel]"));
        assert!(kernel.text.contains("Hard instruction: remain Ene"));
    }
}
