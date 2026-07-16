//! Prompt template management with multi-language support.
//!
//! Prompt strings are loaded from `assets/prompts/{lang}.json` at runtime,
//! keeping all user-facing LLM instructions out of compiled code and enabling
//! future localisation without recompilation.
//!
//! # Usage
//!
//! ```rust,no_run
//! use ene_config::PromptLibrary;
//!
//! let lib = PromptLibrary::load("en");
//! let mascot_context = lib.system().render_mascot_context("Alice", "Bob");
//! ```

use serde::{Deserialize, Serialize};

/// Strongly typed prompt library layout mapping to `assets/prompts/{lang}.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PromptLibraryData {
    /// System prompts configuration.
    pub system: SystemPrompts,
    /// Emotion prompts configuration.
    pub emotion: EmotionPrompts,
    /// Memory prompts configuration.
    pub memory: MemoryPrompts,
    /// Summarizer prompts configuration.
    pub summarizer: SummarizerPrompts,
    /// Session split prompts configuration.
    pub split: SplitPrompts,
    /// Memory extractor prompts configuration.
    pub extractor: ExtractorPrompts,
    /// Affect classifier prompts configuration (#88).
    pub affect_classifier: AffectClassifierPrompts,
}

#[derive(Debug, Deserialize)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawPromptLibraryData {
    system: RawSystemPrompts,
    emotion: RawEmotionPrompts,
    memory: MemoryPrompts,
    summarizer: RawSummarizerPrompts,
    split: SplitPrompts,
    extractor: RawExtractorPrompts,
    #[serde(default)]
    affect_classifier: RawAffectClassifierPrompts,
}

/// Prompt templates for the system prompt.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemPrompts {
    /// Mascot roleplay framing.
    pub mascot_context: String,
    /// Header for rules list.
    pub behavior_rules_header: String,
    /// Header for character identity.
    pub character_header: String,
    /// Header for character personality.
    pub personality_header: String,
    /// Header for character background.
    pub background_header: String,
    /// Header for scene scenario.
    pub scene_header: String,
    /// Header for chat examples.
    pub examples_header: String,
}

#[derive(Debug, Deserialize)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawSystemPrompts {
    mascot_context_path: String,
    behavior_rules_header: String,
    character_header: String,
    personality_header: String,
    background_header: String,
    scene_header: String,
    examples_header: String,
}

impl SystemPrompts {
    /// Renders the mascot context frame replacing `{char_name}` and `{user_name}` placeholders.
    #[must_use]
    pub fn render_mascot_context(&self, char_name: &str, user_name: &str) -> String {
        substitute(
            &self.mascot_context,
            &[("char_name", char_name), ("user_name", user_name)],
        )
    }
}

/// Prompt templates for emotion rules.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmotionPrompts {
    /// Header for emotion output rule.
    pub header: String,
    /// The rule itself.
    pub rule: String,
    /// Token list label.
    pub token_header: String,
    /// Examples list label.
    pub examples_header: String,
    /// Happy example string.
    pub example_happy: String,
    /// Sad example string.
    pub example_sad: String,
    /// Angry example string.
    pub example_angry: String,
    /// Neutral example string.
    pub example_neutral: String,
    /// Natural-dialogue output contract for engine-managed expression (#91).
    pub natural_dialogue_contract: String,
}

#[derive(Debug, Deserialize)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawEmotionPrompts {
    header: String,
    rule_path: String,
    #[serde(default = "default_natural_dialogue_contract_path_en")]
    natural_dialogue_contract_path: String,
    token_header: String,
    examples_header: String,
    example_happy: String,
    example_sad: String,
    example_angry: String,
    example_neutral: String,
}

/// Prompt templates for episodic memory.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryPrompts {
    /// Header for recalled summaries list.
    pub summaries_header: String,
    /// Format template for a summary item.
    pub summary_item: String,
    /// Header for known user facts list.
    pub facts_header: String,
}

impl MemoryPrompts {
    /// Renders a summary item replacing `{age}` and `{text}` placeholders.
    #[must_use]
    pub fn render_summary_item(&self, age: &str, text: &str) -> String {
        substitute(&self.summary_item, &[("age", age), ("text", text)])
    }

    /// Renders the facts header replacing `{user_name}` placeholder.
    #[must_use]
    pub fn render_facts_header(&self, user_name: &str) -> String {
        substitute(&self.facts_header, &[("user_name", user_name)])
    }
}

/// Prompt templates for LLM summarization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SummarizerPrompts {
    /// System prompt for summarizer agent.
    pub system: String,
    /// User prompt template for summarizer agent.
    pub user_prompt: String,
    /// Fallback string when no facts exist.
    pub no_facts_placeholder: String,
}

#[derive(Debug, Deserialize)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawSummarizerPrompts {
    system_path: String,
    user_prompt_path: String,
    no_facts_placeholder: String,
}

impl SummarizerPrompts {
    /// Renders the summarizer system prompt.
    #[must_use]
    pub fn render_system(
        &self,
        user_name: &str,
        char_name: &str,
        existing_facts: &str,
        conversation: &str,
    ) -> String {
        substitute(
            &self.system,
            &[
                ("user_name", user_name),
                ("char_name", char_name),
                ("existing_facts", existing_facts),
                ("conversation", conversation),
            ],
        )
    }

    /// Renders the summarizer user prompt.
    #[must_use]
    pub fn render_user_prompt(
        &self,
        user_name: &str,
        existing_facts: &str,
        conversation: &str,
    ) -> String {
        substitute(
            &self.user_prompt,
            &[
                ("user_name", user_name),
                ("existing_facts", existing_facts),
                ("conversation", conversation),
            ],
        )
    }
}

/// Prompt templates for session split reasons.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SplitPrompts {
    /// Timeout split reason message.
    pub reason_timeout: String,
    /// Topic change split reason message.
    pub reason_topic: String,
    /// Context pressure split reason message.
    pub reason_context: String,
    /// Composite score split reason message.
    pub reason_composite: String,
    /// Manual split reason message.
    pub reason_manual: String,
}

/// Prompt templates for LLM-based memory extraction.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtractorPrompts {
    /// System prompt for the memory extractor agent.
    pub system: String,
    /// User prompt template for the memory extractor agent.
    pub user_prompt: String,
}

#[derive(Debug, Deserialize)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawExtractorPrompts {
    system_path: String,
    user_prompt_path: String,
}

impl ExtractorPrompts {
    /// Renders the extractor user prompt replacing `{conversation}` and
    /// `{pattern_hints}` placeholders.
    #[must_use]
    pub fn render_user_prompt(&self, conversation: &str, pattern_hints: &str) -> String {
        substitute(
            &self.user_prompt,
            &[
                ("conversation", conversation),
                ("pattern_hints", pattern_hints),
            ],
        )
    }
}

/// Prompt templates for the LLM affect classifier (#88).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AffectClassifierPrompts {
    /// System prompt for the affect classifier.
    pub system: String,
    /// User prompt template for the affect classifier.
    pub user_prompt: String,
}

#[derive(Debug, Deserialize, Default)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawAffectClassifierPrompts {
    #[serde(default = "default_affect_classifier_system_path")]
    system_path: String,
    #[serde(default = "default_affect_classifier_user_path")]
    user_prompt_path: String,
}

fn default_affect_classifier_system_path() -> String {
    "en/affect_classifier/system.md".into()
}

fn default_affect_classifier_user_path() -> String {
    "en/affect_classifier/user_prompt.md".into()
}

fn default_natural_dialogue_contract_path_en() -> String {
    "en/emotion/natural_dialogue_contract.md".into()
}

impl AffectClassifierPrompts {
    /// Renders the classifier user prompt replacing `{current_affect}` and `{conversation}`.
    #[must_use]
    pub fn render_user_prompt(&self, current_affect: &str, conversation: &str) -> String {
        substitute(
            &self.user_prompt,
            &[
                ("current_affect", current_affect),
                ("conversation", conversation),
            ],
        )
    }
}

impl SplitPrompts {
    /// Renders the split reason timeout message.
    #[must_use]
    pub fn render_reason_timeout(&self, minutes: &str) -> String {
        substitute(&self.reason_timeout, &[("minutes", minutes)])
    }

    /// Renders the split reason topic change message.
    #[must_use]
    pub fn render_reason_topic(&self, similarity: &str) -> String {
        substitute(&self.reason_topic, &[("similarity", similarity)])
    }

    /// Renders the split reason composite score message.
    #[must_use]
    pub fn render_reason_composite(&self, score: &str) -> String {
        substitute(&self.reason_composite, &[("score", score)])
    }
}

/// Loads and accesses prompt templates from a JSON locale file.
#[derive(Debug, Clone)]
pub struct PromptLibrary {
    data: PromptLibraryData,
    lang: String,
}

impl PromptLibrary {
    /// Loads the prompt library for the given language code (e.g. `"en"`).
    ///
    /// Falls back to the built-in English defaults if the language code is not supported.
    #[must_use]
    pub fn load(lang: &str) -> Self {
        match lang {
            "ja" | "jp" => Self::built_in_japanese(),
            _ => Self::built_in_english(),
        }
    }

    /// Returns the built-in compile-time English defaults.
    ///
    /// These are the same strings shipped in `assets/prompts/en.json` but
    /// embedded at compile time as a fallback so the application works even
    /// when assets are missing (e.g. during unit tests or CI).
    #[must_use]
    pub fn built_in_english() -> Self {
        // The bundled JSON is checked into the repository and is part of the
        // build artifact. A parse failure here is a release-blocker bug, not
        // a runtime condition we can recover from.
        #[expect(
            clippy::expect_used,
            reason = "bundled JSON is validated at build time; parse failure is a release-blocker"
        )]
        let raw: RawPromptLibraryData = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/prompts/en.json"
        )))
        .expect("built-in en.json is always valid");

        let system = SystemPrompts {
            mascot_context: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/en/system/mascot_context.md"
            ))
            .to_string(),
            behavior_rules_header: raw.system.behavior_rules_header,
            character_header: raw.system.character_header,
            personality_header: raw.system.personality_header,
            background_header: raw.system.background_header,
            scene_header: raw.system.scene_header,
            examples_header: raw.system.examples_header,
        };

        let emotion = EmotionPrompts {
            header: raw.emotion.header,
            rule: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/en/emotion/rule.md"
            ))
            .to_string(),
            token_header: raw.emotion.token_header,
            examples_header: raw.emotion.examples_header,
            example_happy: raw.emotion.example_happy,
            example_sad: raw.emotion.example_sad,
            example_angry: raw.emotion.example_angry,
            example_neutral: raw.emotion.example_neutral,
            natural_dialogue_contract: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/en/emotion/natural_dialogue_contract.md"
            ))
            .to_string(),
        };

        let summarizer = SummarizerPrompts {
            system: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/en/summarizer/system.md"
            ))
            .to_string(),
            user_prompt: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/en/summarizer/user_prompt.md"
            ))
            .to_string(),
            no_facts_placeholder: raw.summarizer.no_facts_placeholder,
        };

        let data = PromptLibraryData {
            system,
            emotion,
            memory: raw.memory,
            summarizer,
            split: raw.split,
            extractor: ExtractorPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/en/extractor/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/en/extractor/user_prompt.md"
                ))
                .to_string(),
            },
            affect_classifier: AffectClassifierPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/en/affect_classifier/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/en/affect_classifier/user_prompt.md"
                ))
                .to_string(),
            },
        };

        Self {
            data,
            lang: "en".to_string(),
        }
    }

    /// Returns the built-in compile-time Japanese defaults.
    ///
    /// These are the same strings shipped in `assets/prompts/ja.json` but
    /// embedded at compile time as a fallback.
    #[must_use]
    pub fn built_in_japanese() -> Self {
        // The bundled JSON is checked into the repository and is part of the
        // build artifact. A parse failure here is a release-blocker bug, not
        // a runtime condition we can recover from.
        #[expect(
            clippy::expect_used,
            reason = "bundled JSON is validated at build time; parse failure is a release-blocker"
        )]
        let raw: RawPromptLibraryData = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/prompts/ja.json"
        )))
        .expect("built-in ja.json is always valid");

        let system = SystemPrompts {
            mascot_context: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/ja/system/mascot_context.md"
            ))
            .to_string(),
            behavior_rules_header: raw.system.behavior_rules_header,
            character_header: raw.system.character_header,
            personality_header: raw.system.personality_header,
            background_header: raw.system.background_header,
            scene_header: raw.system.scene_header,
            examples_header: raw.system.examples_header,
        };

        let emotion = EmotionPrompts {
            header: raw.emotion.header,
            rule: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/ja/emotion/rule.md"
            ))
            .to_string(),
            token_header: raw.emotion.token_header,
            examples_header: raw.emotion.examples_header,
            example_happy: raw.emotion.example_happy,
            example_sad: raw.emotion.example_sad,
            example_angry: raw.emotion.example_angry,
            example_neutral: raw.emotion.example_neutral,
            natural_dialogue_contract: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/ja/emotion/natural_dialogue_contract.md"
            ))
            .to_string(),
        };

        let summarizer = SummarizerPrompts {
            system: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/ja/summarizer/system.md"
            ))
            .to_string(),
            user_prompt: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/ja/summarizer/user_prompt.md"
            ))
            .to_string(),
            no_facts_placeholder: raw.summarizer.no_facts_placeholder,
        };

        let data = PromptLibraryData {
            system,
            emotion,
            memory: raw.memory,
            summarizer,
            split: raw.split,
            extractor: ExtractorPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/ja/extractor/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/ja/extractor/user_prompt.md"
                ))
                .to_string(),
            },
            affect_classifier: AffectClassifierPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/ja/affect_classifier/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/ja/affect_classifier/user_prompt.md"
                ))
                .to_string(),
            },
        };

        Self {
            data,
            lang: "ja".to_string(),
        }
    }

    /// Returns the language code this library was loaded for.
    #[must_use]
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Returns reference to system prompts.
    #[must_use]
    pub const fn system(&self) -> &SystemPrompts {
        &self.data.system
    }

    /// Returns reference to emotion prompts.
    #[must_use]
    pub const fn emotion(&self) -> &EmotionPrompts {
        &self.data.emotion
    }

    /// Returns reference to memory prompts.
    #[must_use]
    pub const fn memory(&self) -> &MemoryPrompts {
        &self.data.memory
    }

    /// Returns reference to summarizer prompts.
    #[must_use]
    pub const fn summarizer(&self) -> &SummarizerPrompts {
        &self.data.summarizer
    }

    /// Returns reference to split prompts.
    #[must_use]
    pub const fn split(&self) -> &SplitPrompts {
        &self.data.split
    }

    /// Returns reference to extractor prompts.
    #[must_use]
    pub const fn extractor(&self) -> &ExtractorPrompts {
        &self.data.extractor
    }

    /// Returns reference to affect classifier prompts.
    #[must_use]
    pub const fn affect_classifier(&self) -> &AffectClassifierPrompts {
        &self.data.affect_classifier
    }
}

/// Substitutes `{variable_name}` placeholders in `template` with the provided
/// `(name, value)` pairs. Unknown variables are left untouched.
#[must_use]
pub fn substitute(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in vars {
        result = result.replace(&format!("{{{name}}}"), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_known_vars() {
        let result = substitute(
            "Hello {user_name}, I am {char_name}!",
            &[("user_name", "Alice"), ("char_name", "Alicia")],
        );
        assert_eq!(result, "Hello Alice, I am Alicia!");
    }

    #[test]
    fn substitute_leaves_unknown_vars() {
        let result = substitute("Hello {unknown}!", &[]);
        assert_eq!(result, "Hello {unknown}!");
    }

    #[test]
    fn built_in_english_loads() {
        let lib = PromptLibrary::built_in_english();
        assert!(!lib.system().mascot_context.is_empty());
        assert!(!lib.emotion().header.is_empty());
        assert!(!lib.summarizer().system.is_empty());
    }

    #[test]
    fn render_substitutes_vars_in_nested_key() {
        let lib = PromptLibrary::built_in_english();
        let rendered = lib.memory().render_facts_header("Alice");
        assert!(rendered.contains("Alice"), "rendered: {rendered}");
    }

    #[test]
    fn test_prompt_variables_validation() {
        // Test compile-time embedded english prompts
        let lib_en = PromptLibrary::built_in_english();
        verify_library_variables(&lib_en, "built-in en");

        // Test compile-time embedded japanese prompts
        let lib_ja = PromptLibrary::built_in_japanese();
        verify_library_variables(&lib_ja, "built-in ja");
    }

    fn verify_library_variables(lib: &PromptLibrary, source: &str) {
        // mascot_context must contain variables for character and user names
        let mascot = &lib.system().mascot_context;
        assert!(
            mascot.contains("{char_name}"),
            "[{source}] mascot_context template must contain {{char_name}}"
        );
        assert!(
            mascot.contains("{user_name}"),
            "[{source}] mascot_context template must contain {{user_name}}"
        );

        // summarizer.system must contain variables for character and user names
        let sum_sys = &lib.summarizer().system;
        assert!(
            sum_sys.contains("{char_name}"),
            "[{source}] summarizer.system template must contain {{char_name}}"
        );
        assert!(
            sum_sys.contains("{user_name}"),
            "[{source}] summarizer.system template must contain {{user_name}}"
        );

        // summarizer.user_prompt must contain variables for user_name, existing_facts, and conversation
        let sum_user = &lib.summarizer().user_prompt;
        assert!(
            sum_user.contains("{user_name}"),
            "[{source}] summarizer.user_prompt template must contain {{user_name}}"
        );
        assert!(
            sum_user.contains("{existing_facts}"),
            "[{source}] summarizer.user_prompt template must contain {{existing_facts}}"
        );
        assert!(
            sum_user.contains("{conversation}"),
            "[{source}] summarizer.user_prompt template must contain {{conversation}}"
        );
    }
}
