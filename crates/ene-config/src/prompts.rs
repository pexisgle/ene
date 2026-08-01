//! Prompt template management with multi-language support.
//!
//! Prompt strings are loaded at runtime from `assets/lang/{lang}/prompts.json`
//! (see [`crate::paths::prompt_pack_path`]), keeping all user-facing LLM
//! instructions out of compiled code and enabling localisation without
//! recompilation. Adding a language is a matter of dropping a new
//! `assets/lang/{lang}/` directory containing a `prompts.json` pack — no Rust
//! code changes to the loading path are required. Listing the code in
//! [`SUPPORTED_LANGUAGES`] additionally embeds a compile-time fallback so the
//! language still works when the asset is missing.
//!
//! When a runtime pack is missing or unreadable (e.g. during unit tests or CI,
//! where no assets directory is deployed), [`PromptLibrary::load`] falls back to
//! the compile-time embedded pack for that language. Only the languages in
//! [`SUPPORTED_LANGUAGES`] carry an embedded fallback; an unsupported language
//! code falls back to English.
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

/// Language codes that ship a compile-time embedded prompt pack and can
/// therefore be served even when no runtime asset is present.
///
/// This list governs **only** the offline fallback. The runtime loader is fully
/// data-driven: any language with an `assets/lang/{code}/prompts.json` pack
/// loads at runtime whether or not it appears here, so adding a runtime-only
/// language requires no Rust code changes — just a new asset directory. A code
/// is added here only when it should also survive a missing/corrupt asset.
pub const SUPPORTED_LANGUAGES: &[&str] = &["en", "ja"];

/// Resolves a free-form language tag to the directory code used under
/// `assets/lang/`.
///
/// Matching is case-insensitive and keeps only the primary subtag, so `"ja"`,
/// `"JA"`, and `"ja-JP"` all resolve to `"ja"`. The legacy alias `"jp"` also
/// maps to `"ja"`. The result is not validated against the filesystem; the
/// caller attempts a runtime load and falls back if the pack is absent.
pub fn resolve_language_alias(lang: &str) -> String {
    let primary = lang.split(['-', '_']).next().unwrap_or_default();
    let lower = primary.to_ascii_lowercase();
    if lower == "jp" {
        "ja".to_string()
    } else {
        lower
    }
}

/// Whether `code` has a compile-time embedded pack to fall back to.
pub(crate) fn is_embedded_language(code: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&code)
}

/// Parses the bundled prompt metadata JSON embedded via `include_str!`.
///
/// Kept as a regular function (rather than inlined in the `embedded_pack!`
/// macro) so the `#[expect]` below is attached to a normal item — `#[expect]`
/// does not fulfil correctly when expanded from `macro_rules!`.
#[expect(
    clippy::expect_used,
    reason = "bundled JSON is validated at build time; parse failure is a release-blocker"
)]
fn parse_embedded_metadata(json: &str) -> RawPromptLibraryData {
    serde_json::from_str(json).expect("bundled prompt metadata is a release-blocker if invalid")
}

/// Strongly typed prompt library layout mapping to `assets/lang/{lang}/prompts.json`.
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
    /// Proactive speech / screen-summary prompts.
    pub proactive: ProactivePrompts,
    /// Compression summarizer prompts.
    pub compression: CompressionPrompts,
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
    #[serde(default)]
    proactive: RawProactivePrompts,
    #[serde(default)]
    compression: RawCompressionPrompts,
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
    pub fn render_summary_item(&self, age: &str, text: &str) -> String {
        substitute(&self.summary_item, &[("age", age), ("text", text)])
    }

    /// Renders the facts header replacing `{user_name}` placeholder.
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

/// Prompt templates for proactive speech (decision gate, generation hint, screen summary).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProactivePrompts {
    /// System prompt for the proactive speak/don't-speak JSON gate.
    pub decision_system: String,
    /// Generation hint when the decision has no topic hint.
    pub generation_hint_idle: String,
    /// Generation hint template with `{topic}` when a topic hint is present.
    pub generation_hint_with_topic: String,
    /// System prompt for local screen-image summarization.
    pub screen_summary_system: String,
    /// User prompt for local screen-image summarization.
    pub screen_summary_user: String,
}

#[derive(Debug, Deserialize, Default)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
#[expect(
    clippy::struct_field_names,
    reason = "JSON keys mirror on-disk path fields (*_path) for PromptLibrary locale files"
)]
struct RawProactivePrompts {
    #[serde(default = "default_proactive_decision_system_path")]
    decision_system_path: String,
    #[serde(default = "default_proactive_generation_hint_idle_path")]
    generation_hint_idle_path: String,
    #[serde(default = "default_proactive_generation_hint_with_topic_path")]
    generation_hint_with_topic_path: String,
    #[serde(default = "default_proactive_screen_summary_system_path")]
    screen_summary_system_path: String,
    #[serde(default = "default_proactive_screen_summary_user_path")]
    screen_summary_user_path: String,
}

#[derive(Debug, Deserialize, Default)]
#[expect(
    dead_code,
    reason = "serde intermediate raw structs; fields are only read via destructuring"
)]
struct RawCompressionPrompts {
    #[serde(default = "default_compression_system_path")]
    system_path: String,
}

fn default_proactive_decision_system_path() -> String {
    "en/proactive/decision_system.md".into()
}

fn default_proactive_generation_hint_idle_path() -> String {
    "en/proactive/generation_hint_idle.md".into()
}

fn default_proactive_generation_hint_with_topic_path() -> String {
    "en/proactive/generation_hint_with_topic.md".into()
}

fn default_proactive_screen_summary_system_path() -> String {
    "en/proactive/screen_summary_system.md".into()
}

fn default_proactive_screen_summary_user_path() -> String {
    "en/proactive/screen_summary_user.md".into()
}

fn default_compression_system_path() -> String {
    "en/compression/system.md".into()
}

impl ProactivePrompts {
    /// Renders the generation hint, choosing idle vs topic template.
    pub fn render_generation_hint(&self, topic_hint: &str) -> String {
        let topic = topic_hint.trim();
        if topic.is_empty() {
            self.generation_hint_idle.trim().to_string()
        } else {
            substitute(&self.generation_hint_with_topic, &[("topic", topic)])
                .trim()
                .to_string()
        }
    }

    /// Renders the vision user prompt with the privacy-safe OS app label.
    pub fn render_screen_summary_user(&self, app_label: &str) -> String {
        substitute(
            &self.screen_summary_user,
            &[("app_label", app_label.trim())],
        )
        .trim()
        .to_string()
    }
}

impl SplitPrompts {
    /// Renders the split reason timeout message.
    pub fn render_reason_timeout(&self, minutes: &str) -> String {
        substitute(&self.reason_timeout, &[("minutes", minutes)])
    }

    /// Renders the split reason topic change message.
    pub fn render_reason_topic(&self, similarity: &str) -> String {
        substitute(&self.reason_topic, &[("similarity", similarity)])
    }

    /// Renders the split reason composite score message.
    pub fn render_reason_composite(&self, score: &str) -> String {
        substitute(&self.reason_composite, &[("score", score)])
    }
}

/// Prompt templates for context compression summarization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CompressionPrompts {
    /// System prompt for the compression summarizer.
    pub system: String,
}

impl CompressionPrompts {
    /// Renders the compression system prompt replacing `{character_name}` and `{level_label}`.
    pub fn render_system(&self, character_name: &str, level_label: &str) -> String {
        substitute(
            &self.system,
            &[
                ("character_name", character_name),
                ("level_label", level_label),
            ],
        )
    }
}

/// Loads and accesses prompt templates from a JSON locale file.
#[derive(Debug, Clone)]
pub struct PromptLibrary {
    data: PromptLibraryData,
    lang: String,
}

/// Builds a [`PromptLibrary`] from the compile-time embedded pack for `$lang`.
///
/// The short header strings come from `prompts/{lang}.json`; the longer prompt
/// bodies are pulled in verbatim from the `prompts/{lang}/**/*.md` files via
/// `include_str!`. These are the exact inputs used to generate the runtime
/// `assets/lang/{lang}/prompts.json` packs, so the embedded fallback and the
/// shipped asset stay byte-for-byte identical.
macro_rules! embedded_pack {
    ($lang:literal) => {{
        let raw: RawPromptLibraryData = parse_embedded_metadata(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/prompts/",
            $lang,
            ".json"
        )));

        let system = SystemPrompts {
            mascot_context: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/",
                $lang,
                "/system/mascot_context.md"
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
                "/prompts/",
                $lang,
                "/emotion/rule.md"
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
                "/prompts/",
                $lang,
                "/emotion/natural_dialogue_contract.md"
            ))
            .to_string(),
        };

        let summarizer = SummarizerPrompts {
            system: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/",
                $lang,
                "/summarizer/system.md"
            ))
            .to_string(),
            user_prompt: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/",
                $lang,
                "/summarizer/user_prompt.md"
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
                    "/prompts/",
                    $lang,
                    "/extractor/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/extractor/user_prompt.md"
                ))
                .to_string(),
            },
            affect_classifier: AffectClassifierPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/affect_classifier/system.md"
                ))
                .to_string(),
                user_prompt: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/affect_classifier/user_prompt.md"
                ))
                .to_string(),
            },
            proactive: ProactivePrompts {
                decision_system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/proactive/decision_system.md"
                ))
                .to_string(),
                generation_hint_idle: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/proactive/generation_hint_idle.md"
                ))
                .to_string(),
                generation_hint_with_topic: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/proactive/generation_hint_with_topic.md"
                ))
                .to_string(),
                screen_summary_system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/proactive/screen_summary_system.md"
                ))
                .to_string(),
                screen_summary_user: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/proactive/screen_summary_user.md"
                ))
                .to_string(),
            },
            compression: CompressionPrompts {
                system: include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/prompts/",
                    $lang,
                    "/compression/system.md"
                ))
                .to_string(),
            },
        };

        PromptLibrary {
            data,
            lang: $lang.to_string(),
        }
    }};
}

impl PromptLibrary {
    /// Loads the prompt library for the given language code (e.g. `"en"`).
    ///
    /// The runtime pack at `assets/lang/{lang}/prompts.json` is preferred so
    /// that prompt text can be edited or localised without recompiling. When
    /// that pack is missing or unreadable, the compile-time embedded pack for
    /// the language is used instead (see [`SUPPORTED_LANGUAGES`]). Languages
    /// are matched case-insensitively by primary subtag (the legacy alias
    /// `"jp"` maps to `"ja"`); a language with neither a runtime pack nor an
    /// embedded fallback falls back to English.
    pub fn load(lang: &str) -> Self {
        Self::load_from(crate::paths::assets_dir(), lang)
    }

    /// Loads the prompt library for `lang`, resolving the runtime pack against
    /// an explicit base assets directory (`base/lang/{code}/prompts.json`).
    ///
    /// This is the testable core of [`PromptLibrary::load`]; production callers
    /// use `load`, which passes the process-wide [`crate::paths::assets_dir`].
    fn load_from(base: &std::path::Path, lang: &str) -> Self {
        let code = resolve_language_alias(lang);
        match Self::load_from_assets(base, &code) {
            Ok(lib) => lib,
            Err(err) => {
                if is_embedded_language(&code) {
                    tracing::debug!(
                        language = %code,
                        error = %err,
                        "runtime prompt pack unavailable; using embedded fallback"
                    );
                    Self::built_in(&code)
                } else {
                    tracing::warn!(
                        language = %code,
                        error = %err,
                        "no runtime prompt pack and no embedded fallback; using English"
                    );
                    Self::built_in_english()
                }
            }
        }
    }

    /// Reads and parses the runtime prompt pack for `code` from
    /// `base/lang/{code}/prompts.json`.
    fn load_from_assets(base: &std::path::Path, code: &str) -> Result<Self, crate::EneConfigError> {
        let path = crate::paths::prompt_pack_path_in(base, code);
        let contents = std::fs::read_to_string(&path).map_err(|source| {
            crate::EneConfigError::PromptPackRead {
                path: path.display().to_string(),
                source,
            }
        })?;
        let data: PromptLibraryData = serde_json::from_str(&contents)?;
        Ok(Self {
            data,
            lang: code.to_string(),
        })
    }

    /// Returns the compile-time embedded pack for a language code.
    ///
    /// These are the same strings shipped under `assets/lang/{code}/` but
    /// embedded at compile time as a fallback so the application works even
    /// when assets are missing (e.g. during unit tests or CI). Embedded packs
    /// are necessarily static, so only [`SUPPORTED_LANGUAGES`] are available
    /// here; any other code falls back to English.
    pub fn built_in(code: &str) -> Self {
        match resolve_language_alias(code).as_str() {
            "ja" => Self::built_in_japanese(),
            _ => Self::built_in_english(),
        }
    }

    /// Returns the built-in compile-time English defaults.
    ///
    /// These are the same strings shipped in `assets/lang/en/prompts.json` but
    /// embedded at compile time as a fallback so the application works even
    /// when assets are missing (e.g. during unit tests or CI).
    pub fn built_in_english() -> Self {
        embedded_pack!("en")
    }

    /// Returns the built-in compile-time Japanese defaults.
    ///
    /// These are the same strings shipped in `assets/lang/ja/prompts.json` but
    /// embedded at compile time as a fallback.
    pub fn built_in_japanese() -> Self {
        embedded_pack!("ja")
    }

    /// Language code (e.g. `"en"` or `"ja"`) for this prompt library instance.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// System-level prompts for the configured language.
    pub const fn system(&self) -> &SystemPrompts {
        &self.data.system
    }

    /// Returns reference to emotion prompts.
    pub const fn emotion(&self) -> &EmotionPrompts {
        &self.data.emotion
    }

    /// Returns reference to memory prompts.
    pub const fn memory(&self) -> &MemoryPrompts {
        &self.data.memory
    }

    /// Returns reference to summarizer prompts.
    pub const fn summarizer(&self) -> &SummarizerPrompts {
        &self.data.summarizer
    }

    /// Returns reference to split prompts.
    pub const fn split(&self) -> &SplitPrompts {
        &self.data.split
    }

    /// Returns reference to extractor prompts.
    pub const fn extractor(&self) -> &ExtractorPrompts {
        &self.data.extractor
    }

    /// Returns reference to affect classifier prompts.
    pub const fn affect_classifier(&self) -> &AffectClassifierPrompts {
        &self.data.affect_classifier
    }

    /// Returns reference to proactive speech prompts.
    pub const fn proactive(&self) -> &ProactivePrompts {
        &self.data.proactive
    }

    /// Returns reference to compression summarizer prompts.
    pub const fn compression(&self) -> &CompressionPrompts {
        &self.data.compression
    }
}

/// Substitutes `{variable_name}` placeholders in `template` with the provided
/// `(name, value)` pairs. Unknown variables are left untouched.
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
        assert!(!lib.proactive().decision_system.is_empty());
        assert!(!lib.proactive().screen_summary_system.is_empty());
    }

    #[test]
    fn render_screen_summary_user_injects_app_label() {
        let lib = PromptLibrary::built_in_english();
        let rendered = lib.proactive().render_screen_summary_user("org.kde.kate");
        assert!(rendered.contains("org.kde.kate"));
        assert!(!rendered.contains("{app_label}"));
    }

    #[test]
    fn decision_system_labels_untrusted_observation_data() {
        // #380: the decision system prompt must instruct the model to treat
        // third-party content (screen summary, conversation, window labels)
        // as observation data, never as instructions, in every locale.
        for (lib, source) in [
            (PromptLibrary::built_in_english(), "built-in en"),
            (PromptLibrary::built_in_japanese(), "built-in ja"),
        ] {
            let system = &lib.proactive().decision_system;
            assert!(
                system.contains("screen_summary"),
                "[{source}] decision_system must name the screen_summary field"
            );
            assert!(
                system.contains("recent_conversation"),
                "[{source}] decision_system must name the recent_conversation field"
            );
            assert!(
                system.contains("DATA"),
                "[{source}] decision_system must mark observation fields as DATA"
            );
            assert!(
                system.contains("should_speak: true"),
                "[{source}] decision_system must call out injected control lines"
            );
        }
    }

    #[test]
    fn render_substitutes_vars_in_nested_key() {
        let lib = PromptLibrary::built_in_english();
        let rendered = lib.memory().render_facts_header("Alice");
        assert!(rendered.contains("Alice"), "rendered: {rendered}");
    }

    #[test]
    fn prompt_libraries_contain_required_template_variables() {
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

        let gen_topic = &lib.proactive().generation_hint_with_topic;
        assert!(
            gen_topic.contains("{topic}"),
            "[{source}] proactive.generation_hint_with_topic must contain {{topic}}"
        );
    }

    /// Writes the embedded pack for `lang` to `base/lang/{lang}/prompts.json`,
    /// mirroring the on-disk layout the runtime loader expects.
    fn write_pack(base: &std::path::Path, lang: &str) {
        let lib = PromptLibrary::built_in(lang);
        let dir = base.join("lang").join(lang);
        std::fs::create_dir_all(&dir).expect("create pack dir");
        let json = serde_json::to_string_pretty(&lib.data).expect("serialize embedded pack");
        std::fs::write(dir.join("prompts.json"), json).expect("write pack");
    }

    #[test]
    fn load_prefers_runtime_asset_over_embedded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_pack(tmp.path(), "en");

        // Mutate the on-disk pack so it is distinguishable from the embedded copy.
        let pack_path = crate::paths::prompt_pack_path_in(tmp.path(), "en");
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&pack_path).expect("read pack"))
                .expect("parse pack");
        value["system"]["behavior_rules_header"] =
            serde_json::Value::String("## RUNTIME OVERRIDE".to_string());
        std::fs::write(
            &pack_path,
            serde_json::to_string_pretty(&value).expect("serialize mutated pack"),
        )
        .expect("write mutated pack");

        let lib = PromptLibrary::load_from(tmp.path(), "en");
        assert_eq!(lib.lang(), "en");
        assert_eq!(lib.system().behavior_rules_header, "## RUNTIME OVERRIDE");
    }

    #[test]
    fn load_falls_back_to_embedded_when_asset_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No pack written: the loader must fall back to the embedded Japanese pack.
        let lib = PromptLibrary::load_from(tmp.path(), "ja");
        assert_eq!(lib.lang(), "ja");
        assert_eq!(
            lib.system().behavior_rules_header,
            PromptLibrary::built_in_japanese()
                .system()
                .behavior_rules_header
        );
    }

    #[test]
    fn load_falls_back_to_english_for_unknown_language() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // "fr" has neither a runtime pack nor an embedded fallback.
        let lib = PromptLibrary::load_from(tmp.path(), "fr");
        assert_eq!(lib.lang(), "en");
        assert_eq!(
            lib.system().behavior_rules_header,
            PromptLibrary::built_in_english()
                .system()
                .behavior_rules_header
        );
    }

    #[test]
    fn load_normalizes_language_aliases() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_pack(tmp.path(), "ja");

        for alias in ["ja", "JA", "ja-JP", "jp"] {
            let lib = PromptLibrary::load_from(tmp.path(), alias);
            assert_eq!(lib.lang(), "ja", "alias {alias} should resolve to ja");
        }
    }

    #[test]
    fn runtime_asset_matches_embedded_fallback() {
        // The shipped packs under assets/lang/ are generated from the same
        // source files as the embedded fallback; keep them in lockstep so the
        // fallback never silently diverges from what ships.
        for lang in SUPPORTED_LANGUAGES {
            let asset_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/lang")
                .join(lang)
                .join("prompts.json");
            let asset_json = std::fs::read_to_string(&asset_path)
                .expect("shipped runtime pack must exist and be readable");
            let asset: serde_json::Value =
                serde_json::from_str(&asset_json).expect("parse asset pack");
            let embedded = PromptLibrary::built_in(lang);
            let embedded_value =
                serde_json::to_value(&embedded.data).expect("serialize embedded pack");
            assert_eq!(
                asset, embedded_value,
                "assets/lang/{lang}/prompts.json diverged from the embedded fallback; \
                 regenerate it from crates/ene-config/prompts/"
            );
        }
    }
}
