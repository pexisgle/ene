//! Deterministic `CCv3` → Identity Kernel compilation.
//!
//! The kernel budget is sized from the model's available context window
//! rather than a fixed absolute token count, and truncation counts tokens with
//! a language-aware ratio instead of an English-centric `chars / 4` heuristic.

#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "mind pipeline uses intentional turn/score/index arithmetic; history/token helpers index into bounds-checked conversational buffers"
)]
use ene_config::{
    CharacterCardV3, MacroContext, PolitenessLevel, SpeechLength, UserPersona,
    expand_cbs_macros_ctx,
};

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
/// room. This matches the fixed default so constrained models keep the
/// behaviour they had with a fixed budget.
const MIN_IDENTITY_KERNEL_TOKENS: usize = 400;

/// Ceiling for the kernel budget in tokens.
///
/// A 128K-window model does not need a 16K character definition; past this
/// point extra budget buys nothing and only delays the sections that follow.
const MAX_IDENTITY_KERNEL_TOKENS: usize = 4_000;

/// Size the Identity Kernel token budget from the available context window.
///
/// The budget scales with the window so a large-window model can carry a
/// richer character definition, clamped to
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
    /// in card-derived fields.
    ///
    /// `pick_seed`, when provided, makes `{{pick}}` resolve to a stable choice
    /// for the lifetime of the chat instead of re-rolling on every per-turn
    /// recompilation. Derive it with
    /// [`ene_config::session_pick_seed`] from a session-scoped key.
    ///
    /// `available_window` is the number of prompt tokens the model's context
    /// window leaves for this turn (after the response reserve and safety
    /// margin); the kernel budget is derived from it as a fraction, so larger
    /// models carry a fuller character definition.
    ///
    /// `language` localises the kernel's derived speech-style line (defaults
    /// and structured labels); card-provided speech values keep their own
    /// language.
    #[must_use]
    pub fn compile(
        card: &CharacterCardV3,
        user_name: &str,
        user_persona: Option<&UserPersona>,
        pick_seed: Option<u64>,
        available_window: usize,
        language: &str,
    ) -> IdentityKernel {
        let data = &card.data;
        let char_name = data.get_character_name();
        let max_tokens = identity_kernel_budget_tokens(available_window);

        // One shared context so every field expands identically; `{{pick}}`
        // therefore yields the same option in the personality, background, and
        // scene sections of the same kernel.
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
            // no literal `{{user}}` placeholder leaks to the LLM.
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

        let speech_style = render_speech_style(card, language);

        let anti_impersonation = format!(
            "\n- NEVER speak, act, think, or write for {user_name}. {user_name} controls their own actions.\n\
             - When {user_name} asks you to do something, you may describe YOUR response, not theirs.\n\
             - If {user_name} says something, respond as {char_name}, not as {user_name}."
        );
        // The final line carries the anti-spoofing "Hard instruction:" block.
        // Truncation preserves it structurally (by its position here) rather
        // than by searching for its English marker text, so localising the
        // wording cannot break the guarantee.
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
        // Fit greedily in token space (language-aware), not character space, so
        // a Japanese card is measured against the same budget as an English one.
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

/// Default speech-style phrasing for cards without an
/// [`ene_config::SpeechStyleDefinition`].
///
/// The English wording is the legacy default; the Japanese wording keeps a
/// Japanese character prompt free of English instructions. `ja` is selected by
/// primary subtag so `"ja"`, `"ja-JP"`, and `"jp"` all resolve the same way as
/// [`ene_config::resolve_language_alias`].
fn default_speech_style(lang: &str) -> &'static str {
    if ene_config::resolve_language_alias(lang) == "ja" {
        "自然で温かく、オーバーレイ表示に適した話し方"
    } else {
        "natural, warm, suitable for overlay display"
    }
}

const fn speech_length_phrase(length: SpeechLength, ja: bool) -> &'static str {
    match (length, ja) {
        (SpeechLength::Short, true) => "短めの返答",
        (SpeechLength::Normal, true) => "普通の長さの返答",
        (SpeechLength::Long, true) => "長めの返答",
        (SpeechLength::Short, false) => "short responses",
        (SpeechLength::Normal, false) => "normal-length responses",
        (SpeechLength::Long, false) => "long-form responses",
    }
}

const fn politeness_phrase(politeness: PolitenessLevel, ja: bool) -> &'static str {
    match (politeness, ja) {
        (PolitenessLevel::Casual, true) => "カジュアル",
        (PolitenessLevel::Polite, true) => "丁寧",
        (PolitenessLevel::Formal, true) => "改まった",
        (PolitenessLevel::Casual, false) => "casual",
        (PolitenessLevel::Polite, false) => "polite",
        (PolitenessLevel::Formal, false) => "formal",
    }
}

/// Renders the Identity Kernel's `Speech style` line from the card's
/// [`ene_config::SpeechStyleDefinition`].
///
/// Cards without a definition (or with an empty one) get the concise default.
/// Labels and the default follow `lang`; values keep the card author's language.
fn render_speech_style(card: &CharacterCardV3, lang: &str) -> String {
    let Some(speech) = card.data.get_ene_extension().and_then(|ext| ext.speech) else {
        return default_speech_style(lang).to_string();
    };

    let ja = ene_config::resolve_language_alias(lang) == "ja";
    let mut parts: Vec<String> = Vec::new();
    if let Some(length) = speech.length {
        parts.push(speech_length_phrase(length, ja).to_string());
    }
    for (pronoun, label) in [
        (
            speech.first_person,
            if ja { "一人称" } else { "first person" },
        ),
        (
            speech.second_person,
            if ja { "二人称" } else { "second person" },
        ),
    ] {
        if let Some(pronoun) = pronoun
            && !pronoun.trim().is_empty()
        {
            parts.push(format!("{label} {pronoun}"));
        }
    }
    if let Some(politeness) = speech.politeness {
        parts.push(politeness_phrase(politeness, ja).to_string());
    }
    let verbal_tics: Vec<&str> = speech
        .verbal_tics
        .iter()
        .map(String::as_str)
        .filter(|tic| !tic.trim().is_empty())
        .collect();
    if !verbal_tics.is_empty() {
        let tics = verbal_tics.join(if ja { "、" } else { ", " });
        parts.push(format!(
            "{} {tics}",
            if ja { "口癖" } else { "verbal tics" }
        ));
    }
    if parts.is_empty() {
        return default_speech_style(lang).to_string();
    }
    parts.join(if ja { "；" } else { "; " })
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Truncate the core block to `max_tokens` while always keeping the
/// anti-spoofing `hard_line` intact.
///
/// The hard instruction is identified structurally — it is the dedicated final
/// core line passed in by the caller — rather than by searching for its English
/// marker text, so translating the wording cannot lose the boundary.
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
    use ene_config::{
        CharacterCardV3, EneExtension, PolitenessLevel, SpeechLength, SpeechStyleDefinition,
    };
    use std::fs;
    use std::path::PathBuf;

    fn card_with_speech(speech: SpeechStyleDefinition) -> CharacterCardV3 {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.extensions.ene = Some(EneExtension {
            speech: Some(speech),
            ..EneExtension::default()
        });
        card
    }

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

        // 3_200 → budget floor (400 tokens): core block fills the budget, so the
        // optional scenario section is dropped; 128K leaves room for it.
        let small = CharacterCompiler::compile(&card, "User", None, None, 3_200, "en");
        let large = CharacterCompiler::compile(&card, "User", None, None, 128_000, "en");

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

        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

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
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(kernel.text.contains("Name: Ene"));
        assert!(!kernel.text.contains("Official"));
    }

    #[test]
    fn speech_style_renders_structured_definition_in_english() {
        let card = card_with_speech(SpeechStyleDefinition {
            length: Some(SpeechLength::Short),
            first_person: Some("私".into()),
            second_person: Some("きみ".into()),
            politeness: Some(PolitenessLevel::Casual),
            verbal_tics: vec!["〜だよね".into(), "んだよ".into()],
        });
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(
            kernel.text.contains(
                "Speech style: short responses; first person 私; second person きみ; casual; verbal tics 〜だよね, んだよ"
            ),
            "kernel must render the structured speech style: {}",
            kernel.text
        );
        // The system prompt must not leak into the speech line.
        assert!(!kernel.text.contains("Speech style: Keep"));
    }

    #[test]
    fn speech_style_renders_structured_definition_in_japanese() {
        let card = card_with_speech(SpeechStyleDefinition {
            length: Some(SpeechLength::Long),
            first_person: Some("わたし".into()),
            politeness: Some(PolitenessLevel::Polite),
            verbal_tics: vec!["〜ですわ".into()],
            ..SpeechStyleDefinition::default()
        });
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "ja");

        assert!(
            kernel
                .text
                .contains("Speech style: 長めの返答；一人称 わたし；丁寧；口癖 〜ですわ"),
            "a Japanese card must get Japanese speech-style labels: {}",
            kernel.text
        );
    }

    #[test]
    fn speech_style_without_definition_uses_default_not_system_prompt() {
        // Regression: the old fallback surfaced the system prompt's first 160
        // characters as the speech style, so unrelated instructions appeared
        // under "Speech style:".
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "First, remember the house rules.".repeat(20);
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        let speech_line = kernel
            .text
            .lines()
            .find(|line| line.starts_with("Speech style:"))
            .expect("kernel has a speech style line");
        assert_eq!(
            speech_line,
            "Speech style: natural, warm, suitable for overlay display"
        );
        assert!(
            !speech_line.contains("house rules"),
            "system prompt text must not be used as the speech style: {speech_line}"
        );
    }

    #[test]
    fn speech_style_english_keywords_no_longer_trigger_detection() {
        // Regression: the old keyword probe answered "short, warm, suitable
        // for overlay display" for any prompt containing "short"; the speech
        // line must now come from the card definition or the default only.
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "Keep responses short and brief for the overlay.".into();
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(
            !kernel.text.contains("short, warm"),
            "keyword inference must not fire: {}",
            kernel.text
        );
        assert!(
            kernel
                .text
                .contains("Speech style: natural, warm, suitable for overlay display")
        );
    }

    #[test]
    fn speech_style_default_is_localized() {
        let mut card = CharacterCardV3::default();
        card.data.name = "エネ".into();
        let kernel = CharacterCompiler::compile(&card, "ユーザー", None, None, 400, "ja");

        assert!(
            kernel
                .text
                .contains("Speech style: 自然で温かく、オーバーレイ表示に適した話し方"),
            "a Japanese prompt must not get an English default speech style: {}",
            kernel.text
        );
    }

    #[test]
    fn speech_style_empty_definition_falls_back_to_default() {
        let card = card_with_speech(SpeechStyleDefinition::default());
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(
            kernel
                .text
                .contains("Speech style: natural, warm, suitable for overlay display")
        );
    }

    #[test]
    fn speech_style_empty_values_are_omitted() {
        let card = card_with_speech(SpeechStyleDefinition {
            first_person: Some(String::new()),
            second_person: Some("   ".into()),
            ..SpeechStyleDefinition::default()
        });
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(
            !kernel.text.contains("first person"),
            "blank values must not produce dangling labels: {}",
            kernel.text
        );
        assert!(
            kernel
                .text
                .contains("Speech style: natural, warm, suitable for overlay display")
        );
    }

    #[test]
    fn speech_style_language_aliases_and_fallbacks_are_resolved() {
        let card = CharacterCardV3::default();
        for language in ["ja", "ja-JP", "jp"] {
            let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, language);
            assert!(kernel.text.contains("Speech style: 自然で温かく"));
        }
        for language in ["fr", ""] {
            let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, language);
            assert!(kernel.text.contains("Speech style: natural, warm"));
        }
    }

    #[test]
    fn speech_style_ignores_empty_verbal_tics() {
        let card = card_with_speech(SpeechStyleDefinition {
            verbal_tics: vec![String::new(), "  ".into(), "だよね".into()],
            ..SpeechStyleDefinition::default()
        });
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "ja");
        assert!(kernel.text.contains("口癖 だよね"));
        assert!(!kernel.text.contains("口癖 、"));
    }

    #[test]
    fn macros_expanded_in_kernel() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Friends with {{user}}.".into();
        let kernel = CharacterCompiler::compile(&card, "Alice", None, None, 400, "en");

        assert!(kernel.text.contains("Friends with Alice."));
        // The anti-impersonation guard is expanded at compile time, so no
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
        let kernel = CharacterCompiler::compile(&card, "Alice", Some(&persona), None, 400, "en");

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
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(!kernel.text.contains("Stay in character."));
        let phi = kernel
            .post_history_instructions
            .expect("post_history present");
        assert!(phi.contains("Stay in character."));
        // Anti-impersonation reinforcement is appended and expanded at compile time.
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

    /// The identity kernel is recompiled on every turn, so a seeded `{{pick}}`
    /// must resolve to the same trait each time instead of re-rolling (which
    /// would change hair colour / hometown per turn).
    #[test]
    fn pick_is_stable_across_recompilations_with_seed() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.personality = "Hair color: {{pick:red,blue,green,gold,silver}}.".into();

        let seed = Some(ene_config::session_pick_seed("ene:session-1"));
        let first = CharacterCompiler::compile(&card, "User", None, seed, 400, "en");
        for _ in 0..16 {
            let again = CharacterCompiler::compile(&card, "User", None, seed, 400, "en");
            assert_eq!(
                again.text, first.text,
                "seeded {{{{pick}}}} must be stable across recompilations"
            );
        }
    }

    #[test]
    fn alicia_default_card_compiles() {
        let card = alicia_card();
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 400, "en");

        assert!(kernel.text.contains("Name: Alicia"));
        assert!(kernel.text.contains("cheerful") || kernel.text.contains("Friendly"));
        assert!(kernel.text.contains("Hard instruction: remain Alicia"));
        assert!(kernel.text.contains("Speech style: short responses"));
    }

    #[test]
    fn truncate_drops_optional_sections_before_core_header() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "x".repeat(2_000);
        card.data.description = "y".repeat(2_000);
        // A small window forces truncation; the core header and the
        // anti-spoofing block must both survive.
        let kernel = CharacterCompiler::compile(&card, "User", None, None, 8_000, "en");
        assert!(kernel.text.contains("[Identity Kernel]"));
        assert!(kernel.text.contains("Hard instruction: remain Ene"));
    }

    #[test]
    fn japanese_card_is_not_over_truncated() {
        // A rich Japanese card. Under the `max_tokens * 4` char heuristic a
        // 1,600-char budget would hold only ~400 tokens of Japanese, gutting a
        // card like this; the language-aware count sizes it correctly.
        let mut card = CharacterCardV3::default();
        card.data.name = "エネ".into();
        card.data.personality = "元気いっぱいで、少しお茶目なデスクトップの相棒。".into();
        card.data.description = "彼女は画面の中で暮らし、いつもユーザーを励ましている。".repeat(6);
        card.data.scenario = "今日は一緒に作業しながら、たまに雑談をする場面。".repeat(6);

        let kernel = CharacterCompiler::compile(&card, "ユーザー", None, None, 16_000, "ja");

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
        // marker would still be kept.
        let head_lines = ["[Identity Kernel]".to_string(), "Name: Ene".to_string()];
        let hard_line = "絶対指示: 長い会話でもエネのままでいてください".to_string();

        let truncated = truncate_preserving_core(&head_lines, &hard_line, 40);
        assert!(
            truncated.contains("絶対指示"),
            "a non-English hard instruction must still be preserved: {truncated}"
        );
    }
}
