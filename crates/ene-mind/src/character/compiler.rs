//! Deterministic `CCv3` → Identity Kernel compilation (#82).

use ene_config::{CharacterCardV3, expand_cbs_macros};

use super::kernel::IdentityKernel;

/// Default maximum kernel size in approximate tokens (4 chars/token heuristic).
pub const DEFAULT_IDENTITY_KERNEL_MAX_TOKENS: usize = 400;

/// Compiles `CCv3` character fields into a compact Identity Kernel.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharacterCompiler;

impl CharacterCompiler {
    /// Compile an Identity Kernel from a V3 character card.
    #[must_use]
    pub fn compile(card: &CharacterCardV3, user_name: &str, max_tokens: usize) -> IdentityKernel {
        let data = &card.data;
        let char_name = data.get_character_name();
        let max_chars = max_tokens.saturating_mul(4).max(256);

        let post_history = optional_expanded(&data.post_history_instructions, char_name, user_name);

        let core_personality = if !data.personality.trim().is_empty() {
            expand_field(&data.personality, char_name, user_name)
        } else if !data.description.trim().is_empty() {
            truncate_chars(&expand_field(&data.description, char_name, user_name), 240)
        } else {
            String::from("helpful and consistent")
        };

        let speech_style = derive_speech_style(&data.system_prompt, char_name, user_name);

        let core_lines = [
            "[Identity Kernel]".to_string(),
            format!("Name: {char_name}"),
            format!("Role: desktop companion living on {user_name}'s screen"),
            format!("Core personality: {core_personality}"),
            format!("Speech style: {speech_style}"),
            format!("Hard instruction: remain {char_name} even in long conversations"),
        ];
        let core_block = core_lines.join("\n");

        let mut optional_sections: Vec<String> = Vec::new();

        if !data.system_prompt.trim().is_empty() {
            optional_sections.push(format!(
                "## Core Instructions\n{}",
                expand_field(&data.system_prompt, char_name, user_name)
            ));
        }
        if !data.personality.trim().is_empty() && !data.description.trim().is_empty() {
            optional_sections.push(format!(
                "## Background\n{}",
                expand_field(&data.description, char_name, user_name)
            ));
        }
        if !data.scenario.trim().is_empty() {
            optional_sections.push(format!(
                "## Current Scene\n{}",
                expand_field(&data.scenario, char_name, user_name)
            ));
        }
        if !data.creator_notes.trim().is_empty() {
            optional_sections.push(format!(
                "## Creator Notes\n{}",
                expand_field(&data.creator_notes, char_name, user_name)
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

fn expand_field(raw: &str, char_name: &str, user_name: &str) -> String {
    expand_cbs_macros(raw.trim(), char_name, user_name)
}

fn optional_expanded(raw: &str, char_name: &str, user_name: &str) -> Option<String> {
    if raw.trim().is_empty() {
        None
    } else {
        Some(expand_field(raw, char_name, user_name))
    }
}

fn derive_speech_style(system_prompt: &str, char_name: &str, user_name: &str) -> String {
    let expanded = expand_field(system_prompt, char_name, user_name);
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

        let kernel = CharacterCompiler::compile(&card, "User", 400);
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
        let kernel = CharacterCompiler::compile(&card, "User", 400);
        assert!(kernel.text.contains("Name: Ene"));
        assert!(!kernel.text.contains("Official"));
    }

    #[test]
    fn macros_expanded_in_kernel() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Friends with {{user}}.".into();
        let kernel = CharacterCompiler::compile(&card, "Alice", 400);
        assert!(kernel.text.contains("Friends with Alice."));
        assert!(!kernel.text.contains("{{user}}"));
    }

    #[test]
    fn post_history_instructions_stored_separately() {
        let mut card = CharacterCardV3::default();
        card.data.post_history_instructions = "Stay in character.".into();
        let kernel = CharacterCompiler::compile(&card, "User", 400);
        assert!(!kernel.text.contains("Stay in character."));
        assert_eq!(
            kernel.post_history_instructions.as_deref(),
            Some("Stay in character.")
        );
    }

    #[test]
    fn alicia_default_card_compiles() {
        let card = alicia_card();
        let kernel = CharacterCompiler::compile(&card, "User", 400);
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
        let kernel = CharacterCompiler::compile(&card, "User", 50);
        assert!(kernel.text.contains("[Identity Kernel]"));
        assert!(kernel.text.contains("Hard instruction: remain Ene"));
    }
}
