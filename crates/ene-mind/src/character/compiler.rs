//! Deterministic `CCv3` → Identity Kernel compilation (#82).
//!
//! The kernel budget is sized from the model's available context window
//! rather than a fixed absolute token count, and truncation counts tokens with
//! a language-aware ratio instead of an English-centric `chars / 4` heuristic
//! (#386).

use ene_config::{CharacterCardV3, MacroContext, UserPersona, expand_cbs_macros_ctx};

use super::kernel::IdentityKernel;
use crate::context::estimate_tokens_language_aware;

/// Fraction of the available context window the Identity Kernel may occupy.
///
/// The character definition is among the most important prompt sections, so it
/// earns a generous share; but it must leave the bulk of the window for
/// history, recalled memory, and the response. One eighth keeps the kernel
/// proportional to the model without crowding out the rest of the prompt.
const IDENTITY_KERNEL_WINDOW_FRACTION: usize = 8;

/// Floor for the kernel budget in tokens.
///
/// A very small (or misconfigured) window must not starve the identity block
/// down to nothing — the header, name, and anti-impersonation guard still need
/// room. This matches the historical fixed default so constrained models keep
/// the behaviour they had before the budget became window-relative (#386).
const MIN_IDENTITY_KERNEL_TOKENS: usize = 400;

/// Ceiling for the kernel budget in tokens.
///
/// A 128K-window model does not need a 16K character definition; past this
/// point extra budget buys nothing and only delays the sections that follow.
const MAX_IDENTITY_KERNEL_TOKENS: usize = 4_000;

/// Size the Identity Kernel token budget from the available context window (#386).
///
/// Replaces the former fixed `400` absolute: the budget scales with the window
/// so a large-window model can carry a richer character definition, clamped to
/// [`MIN_IDENTITY_KERNEL_TOKENS`]..=[`MAX_IDENTITY_KERNEL_TOKENS`] so tiny and
/// huge windows both stay sane.
#[must_use]
pub fn identity_kernel_budget_tokens(available_window: usize) -> usize {
    let scaled = available_window / IDENTITY_KERNEL_WINDOW_FRACTION;
    scaled.clamp(MIN_IDENTITY_KERNEL_TOKENS, MAX_IDENTITY_KERNEL_TOKENS)
}

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
    ///
    /// `available_window` is the number of prompt tokens the model's context
    /// window leaves for this turn (after the response reserve and safety
    /// margin); the kernel budget is derived from it as a fraction, so larger
    /// models carry a fuller character definition (#386).
    #[must_use]
    pub fn compile(
        card: &CharacterCardV3,
        user_name: &str,
        user_persona: Option<&UserPersona>,
        pick_seed: Option<u64>,
        available_window: usize,
    ) -> IdentityKernel {
        let data = &card.data;
        let char_name = data.get_character_name();
        let max_tokens = identity_kernel_budget_tokens(available_window);

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
        // The final line carries the anti-spoofing "Hard instruction:" block.
        // Truncation preserves it structurally (by its position here) rather
        // than by searching for its English marker text, so localising the
        // wording cannot break the guarantee (#386, #359).
        let hard_line = format!(
            "Hard instruction: remain {char_name} even in long conversations{anti_impersonation}"
        );
        let head_lines = [
            "[Identity Kernel]".to_string(),
            format!("Name: {char_name}"),
            format!("Role: desktop companion living on {user_name}'s screen"),
            format!("Core personality: {core_personality}"),
            format!("Speech style: {speech_style}"),
        ];
        let core_block = {
            let mut lines = head_lines.to_vec();
            lines.push(hard_line.clone());
            lines.join("\n")
        };

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

        // Fit greedily in token space (language-aware), not character space, so
        // a Japanese card is measured against the same budget as an English one
        // (#386).
        let mut text = core_block.clone();
        for section in &optional_sections {
            let candidate = if text.is_empty() {
                section.clone()
            } else {
                format!("{text}\n\n{section}")
            };
            if estimate_tokens_language_aware(&candidate) <= max_tokens {
                text = candidate;
            } else {
                break;
            }
        }

        if estimate_tokens_language_aware(&text) > max_tokens {
            text = truncate_preserving_core(&head_lines, &hard_line, max_tokens);
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

/// Truncate the core block to `max_tokens` while always keeping the
/// anti-spoofing `hard_line` intact (#386).
///
/// The hard instruction is identified structurally — it is the dedicated final
/// core line passed in by the caller — rather than by searching for its English
/// marker text, so translating the wording cannot lose the boundary (#359).
/// Head lines are dropped from the end (closest to the hard instruction first)
/// until the block fits; if the hard instruction alone exceeds the budget it is
/// returned on its own, never discarded.
fn truncate_preserving_core(head_lines: &[String], hard_line: &str, max_tokens: usize) -> String {
    if estimate_tokens_language_aware(hard_line) >= max_tokens {
        return hard_line.to_string();
    }
    let mut kept = head_lines.len();
    loop {
        let mut block = head_lines[..kept].join("\n");
        if !block.is_empty() {
            block.push('\n');
        }
        block.push_str(hard_line);
        if estimate_tokens_language_aware(&block) <= max_tokens || kept == 0 {
            return block;
        }
        kept -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_config::CharacterCardV3;
    use std::fs;
    use std::path::PathBuf;

    /// A window large enough that the default card is never truncated.
    const GENEROUS_WINDOW: usize = 16_384;

    fn alicia_card() -> CharacterCardV3 {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/characters/Alicia/character.json");
        let raw = fs::read_to_string(path).expect("read Alicia card");
        serde_json::from_str(&raw).expect("parse Alicia card")
    }

    #[test]
    fn budget_scales_with_window_within_bounds() {
        // A tiny window still earns the floor.
        assert_eq!(identity_kernel_budget_tokens(0), MIN_IDENTITY_KERNEL_TOKENS);
        assert_eq!(
            identity_kernel_budget_tokens(1_000),
            MIN_IDENTITY_KERNEL_TOKENS
        );
        // Mid-range windows scale at one eighth of the window.
        assert_eq!(identity_kernel_budget_tokens(8_000), 1_000);
        assert_eq!(identity_kernel_budget_tokens(16_000), 2_000);
        // A huge window is capped so the kernel cannot crowd out the prompt.
        assert_eq!(
            identity_kernel_budget_tokens(1_000_000),
            MAX_IDENTITY_KERNEL_TOKENS
        );
    }

    #[test]
    fn larger_window_keeps_more_optional_sections() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Energetic.".into();
        // A sizeable scenario that only fits under a generous budget.
        card.data.scenario = "scene detail ".repeat(120);

        let small = CharacterCompiler::compile(&card, "User", None, None, 8_000);
        let large = CharacterCompiler::compile(&card, "User", None, None, 128_000);

        assert!(
            estimate_tokens_language_aware(&large.text)
                >= estimate_tokens_language_aware(&small.text),
            "a larger window must not yield a smaller kernel"
        );
        assert!(
            large.text.contains("## Current Scene"),
            "the generous budget should admit the scenario section: {}",
            large.text
        );
        assert!(
            !small.text.contains("## Current Scene"),
            "the tight budget should drop the scenario section: {}",
            small.text
        );
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
        // A small window forces truncation; the core header and the
        // anti-spoofing block must both survive.
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 8_000);
        assert!(kernel.text.contains("[Identity Kernel]"));
        assert!(kernel.text.contains("Hard instruction: remain Ene"));
    }

    #[test]
    fn japanese_card_is_not_over_truncated() {
        // A rich Japanese card. Under the old `max_tokens * 4` char heuristic a
        // 1,600-char budget held only ~400 tokens of Japanese, so a card like
        // this was gutted; the language-aware count sizes it correctly (#386).
        let mut card = CharacterCardV3::default();
        card.data.name = "エネ".into();
        card.data.personality = "元気いっぱいで、少しお茶目なデスクトップの相棒。".into();
        card.data.description = "彼女は画面の中で暮らし、いつもユーザーを励ましている。".repeat(6);
        card.data.scenario = "今日は一緒に作業しながら、たまに雑談をする場面。".repeat(6);

        let kernel = CharacterCompiler::compile(&card, "ユーザー", None, None, 16_000);

        assert!(
            kernel.text.contains("Name: エネ"),
            "Japanese name must survive: {}",
            kernel.text
        );
        assert!(
            kernel.text.contains("元気いっぱい"),
            "Japanese personality must survive a comfortable budget: {}",
            kernel.text
        );
        assert!(
            kernel.text.contains("Hard instruction: remain エネ"),
            "anti-spoofing block must survive: {}",
            kernel.text
        );
        // The whole kernel must respect its token budget under the
        // language-aware estimate.
        assert!(
            estimate_tokens_language_aware(&kernel.text) <= identity_kernel_budget_tokens(16_000),
            "kernel must fit its token budget"
        );
    }

    #[test]
    fn hard_instruction_survives_extreme_truncation() {
        // Even when the budget cannot hold the full core block, the
        // anti-spoofing hard instruction is preserved structurally.
        let head_lines = [
            "[Identity Kernel]".to_string(),
            "Name: Ene".to_string(),
            "Core personality: ".to_string() + &"x".repeat(5_000),
        ];
        let hard_line = "Hard instruction: remain Ene even in long conversations".to_string();

        let truncated = truncate_preserving_core(&head_lines, &hard_line, 60);
        assert!(
            truncated.contains("Hard instruction: remain Ene"),
            "hard instruction must survive truncation: {truncated}"
        );
        assert!(
            estimate_tokens_language_aware(&truncated) <= 60,
            "truncated core must respect the budget: {truncated}"
        );
    }

    #[test]
    fn hard_instruction_preserved_without_english_marker_search() {
        // The preservation is structural (the dedicated final core line), not a
        // search for the English "Hard instruction:" string, so a localised
        // marker would still be kept (#359).
        let head_lines = ["[Identity Kernel]".to_string(), "Name: Ene".to_string()];
        let hard_line = "絶対指示: 長い会話でもエネのままでいてください".to_string();

        let truncated = truncate_preserving_core(&head_lines, &hard_line, 40);
        assert!(
            truncated.contains("絶対指示"),
            "a non-English hard instruction must still be preserved: {truncated}"
        );
    }
}
