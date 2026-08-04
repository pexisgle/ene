use chrono::{DateTime, Datelike, Local};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A V3-format character card following the
/// [Character Card Spec V3](https://github.com/kwaroran/character-card-spec-v3).
pub struct CharacterCardV3 {
    /// Spec identifier (e.g. `"chara_card_v3"`).
    #[serde(default = "default_spec")]
    pub spec: String,
    /// Spec version (e.g. `"3.0"`).
    #[serde(default = "default_spec_version")]
    pub spec_version: String,
    /// The card's data payload.
    pub data: CharacterCardData,
}

fn default_spec() -> String {
    "chara_card_v3".to_string()
}

fn default_spec_version() -> String {
    "3.0".to_string()
}

impl Default for CharacterCardV3 {
    fn default() -> Self {
        Self {
            spec: "chara_card_v3".to_string(),
            spec_version: "3.0".to_string(),
            data: CharacterCardData::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// The core data payload of a V3 character card.
pub struct CharacterCardData {
    /// The character's primary name.
    #[serde(default)]
    pub name: String,
    /// A short description of the character.
    #[serde(default)]
    pub description: String,
    /// Tags / categories for discovery.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Who created this card.
    #[serde(default)]
    pub creator: String,
    /// Version string for this character definition.
    #[serde(default)]
    pub character_version: String,
    /// Example dialogue shown to the LLM on the first turn.
    #[serde(default)]
    pub mes_example: String,
    /// Extension key-value store (expressions, ene metadata, etc.).
    #[serde(default)]
    pub extensions: Extensions,
    /// The character's system prompt.
    #[serde(default)]
    pub system_prompt: String,
    /// Instructions appended after the conversation history (PHI).
    #[serde(default)]
    pub post_history_instructions: String,
    /// The character's opening message.
    #[serde(default)]
    pub first_mes: String,
    /// Alternate greeting messages that can replace `first_mes`.
    #[serde(default)]
    pub alternate_greetings: Vec<String>,
    /// Personality traits description.
    #[serde(default)]
    pub personality: String,
    /// Scenario / setting description.
    #[serde(default)]
    pub scenario: String,
    /// Notes from the card creator (`CCv2`+).
    #[serde(default)]
    pub creator_notes: String,
    /// Optional lorebook for world-building context.
    #[serde(default)]
    pub character_book: Option<Lorebook>,
    /// References to external assets (VRM, VRMA, etc.).
    #[serde(default)]
    pub assets: Vec<CharacterAsset>,
    /// An alternative display name (preferred over `name` when non-empty).
    #[serde(default)]
    pub nickname: String,
    /// Multilingual creator notes.
    #[serde(default)]
    pub creator_notes_multilingual: Option<HashMap<String, String>>,
    /// Attribution sources for the card.
    #[serde(default)]
    pub source: Option<Vec<String>>,
    /// Alternative greetings shown only in group chats.
    ///
    /// Parsed and preserved for cards authored against the `CCv3` spec, but
    /// unused: Ene renders a single character, so group-chat greetings have
    /// no consumer. The field becomes meaningful if Ene ever displays
    /// multiple characters at once.
    #[serde(default)]
    pub group_only_greetings: Vec<String>,
    /// Unix timestamp of when the card was created.
    #[serde(default)]
    pub creation_date: Option<u64>,
    /// Unix timestamp of the last modification.
    #[serde(default)]
    pub modification_date: Option<u64>,
    /// Author's note: persistent instruction injected at a specific depth in the conversation.
    /// Unlike the system prompt, this sits within the history at depth N from the end,
    /// keeping the main system prompt clean while enforcing late-session behavior.
    #[serde(default)]
    pub authors_note: Option<String>,
    /// Depth at which to insert the author's note from the end of history.
    /// 0 = most recent assistant turn.
    #[serde(default)]
    pub authors_note_depth: Option<usize>,
    /// Catch-all for top-level `data` fields this build does not model.
    ///
    /// Cards produced by other apps may carry vendor-specific keys; keeping
    /// them here lets an edit-and-save round-trip preserve everything instead
    /// of silently dropping it. An [`IndexMap`] (not `HashMap`) so unknown
    /// keys keep their original order on re-serialization.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Typed extension store for character cards.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(crate = "crate::serde")]
pub struct Extensions {
    /// Ene-specific extension block (motions, expressions, etc.).
    #[serde(default, deserialize_with = "deserialize_ene")]
    pub ene: Option<EneExtension>,
    /// Catch-all for other extension keys.
    ///
    /// An [`IndexMap`] (not `HashMap`) so iteration order is deterministic:
    /// `HashMap` reseeds per process and would make saving the same card twice
    /// produce different bytes.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Lenient reader for `extensions.ene`: a missing key, a non-object value
/// (some V2 exporters write arbitrary types here), or a wrong-typed object
/// all yield `None` so one malformed extension cannot sink the whole card.
fn deserialize_ene<'de, D>(deserializer: D) -> Result<Option<EneExtension>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(serde_json::Value::Object(mut map)) = value else {
        return Ok(None);
    };
    let locales = map.remove("locales");
    let mut ene: EneExtension =
        serde_json::from_value(serde_json::Value::Object(map)).unwrap_or_default();
    if let Some(serde_json::Value::Object(locales)) = locales {
        let mut parsed = indexmap::IndexMap::new();
        for (code, value) in locales {
            match serde_json::from_value(value) {
                Ok(diff) => {
                    parsed.insert(code, diff);
                }
                Err(error) => {
                    tracing::warn!(%code, %error, "Skipping malformed localized card diff");
                }
            }
        }
        if !parsed.is_empty() {
            ene.locales = Some(parsed);
        }
    }
    Ok(Some(ene))
}

impl schemars::JsonSchema for Extensions {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Extensions".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let ene_schema = generator.subschema_for::<Option<EneExtension>>();
        let value = serde_json::json!({
            "type": "object",
            "properties": {
                "ene": ene_schema
            },
            "additionalProperties": true
        });
        // The JSON literal above is a known-good object; `from_value` is
        // infallible for this shape.
        #[expect(
            clippy::unwrap_used,
            reason = "known-good JSON literal constructed inline; cannot fail"
        )]
        serde_json::from_value(value).unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A reference to an external asset (VRM model, VRMA animation, etc.).
pub struct CharacterAsset {
    /// The type of asset (e.g. `"vrm"`, `"vrma"`, `"png"`).
    #[serde(default, rename = "type")]
    pub asset_type: String,
    /// URI pointing to the asset file.
    #[serde(default)]
    pub uri: String,
    /// Human-readable name for the asset.
    #[serde(default)]
    pub name: String,
    /// File extension (e.g. `"vrm"`, `"vrma"`).
    #[serde(default)]
    pub ext: String,
    /// Catch-all for vendor asset fields this build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A lorebook (world-info) attached to a character card.
pub struct Lorebook {
    /// Optional name for this lorebook.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional description of the lorebook's purpose.
    #[serde(default)]
    pub description: Option<String>,
    /// How many messages back to scan for trigger keys.
    #[serde(default)]
    pub scan_depth: Option<u32>,
    /// Maximum number of tokens the lorebook entries may consume.
    #[serde(default)]
    pub token_budget: Option<u32>,
    /// Whether scanning should recurse into previously matched entries.
    #[serde(default)]
    pub recursive_scanning: Option<bool>,
    /// Extension data for the lorebook.
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    /// Catch-all for vendor lorebook fields this build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
    /// The list of lorebook entries.
    pub entries: Vec<LorebookEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A single entry inside a lorebook.
pub struct LorebookEntry {
    /// Trigger key-words / phrases that activate this entry.
    pub keys: Vec<String>,
    /// The content injected when this entry is activated.
    pub content: String,
    /// Extension data for this entry.
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    /// Whether this entry is enabled.
    pub enabled: bool,
    /// Positional ordering among entries (lower = earlier).
    pub insertion_order: i32,
    /// Whether key matching is case-sensitive.
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    /// Whether the keys should be treated as regular expressions.
    pub use_regex: bool,
    /// If true, this entry is always injected regardless of key matching.
    #[serde(default)]
    pub constant: Option<bool>,
    /// Optional display name for the entry.
    #[serde(default)]
    pub name: Option<String>,
    /// Priority override for ordering.
    #[serde(default)]
    pub priority: Option<i32>,
    /// Unique identifier (type varies by implementation).
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    /// Free-form comment about this entry.
    #[serde(default)]
    pub comment: Option<String>,
    /// Whether secondary keys are used for matching.
    #[serde(default)]
    pub selective: Option<bool>,
    /// Secondary key-words that must also match.
    #[serde(default)]
    pub secondary_keys: Option<Vec<String>>,
    /// Where the content is inserted (`"before_char"` or `"after_char"`).
    #[serde(default)]
    pub position: Option<String>,
    /// Catch-all for vendor entry fields (e.g. `probability`, `uid`) this
    /// build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// Structured user persona for roleplay context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct UserPersona {
    /// User's display name.
    pub name: String,
    /// Physical description of the user.
    #[serde(default)]
    pub description: Option<String>,
    /// Relationship to the character.
    #[serde(default)]
    pub relationship: Option<String>,
    /// Preferred pronouns.
    #[serde(default)]
    pub pronouns: Option<String>,
    /// Custom notes/additional context about the user.
    #[serde(default)]
    pub notes: Option<String>,
}

impl Default for UserPersona {
    fn default() -> Self {
        Self {
            name: String::from("User"),
            description: None,
            relationship: None,
            pronouns: None,
            notes: None,
        }
    }
}

impl UserPersona {
    /// Render the persona as labeled lines, each prefixed with `line_prefix`.
    ///
    /// Single canonical field rendering shared by CBS `{{user_persona}}` macro
    /// expansion (empty prefix) and prompt-budget injection (`"- "` bullets) so
    /// the two never diverge. Empty optional fields are omitted.
    #[must_use]
    pub fn render_lines(&self, line_prefix: &str) -> String {
        let mut parts = vec![format!("{line_prefix}Name: {}", self.name)];
        if let Some(ref desc) = self.description
            && !desc.trim().is_empty()
        {
            parts.push(format!("{line_prefix}Description: {desc}"));
        }
        if let Some(ref rel) = self.relationship
            && !rel.trim().is_empty()
        {
            parts.push(format!("{line_prefix}Relationship: {rel}"));
        }
        if let Some(ref pron) = self.pronouns
            && !pron.trim().is_empty()
        {
            parts.push(format!("{line_prefix}Pronouns: {pron}"));
        }
        if let Some(ref notes) = self.notes
            && !notes.trim().is_empty()
        {
            parts.push(format!("{line_prefix}Notes: {notes}"));
        }
        parts.join("\n")
    }
}

const fn default_enabled() -> bool {
    true
}

fn vrm_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let schema = <HashMap<String, f32> as schemars::JsonSchema>::json_schema(generator);
    let mut value = serde_json::to_value(&schema).unwrap_or_default();
    if let serde_json::Value::Object(ref mut obj) = value {
        obj.insert("minProperties".to_string(), serde_json::json!(1));
    }
    serde_json::from_value(value).unwrap_or(schema)
}

/// Affect point an expression maps to, used for affect-to-expression resolution.
///
/// A card author places each expression in affect space; the runtime picks the
/// nearest annotated expression to the current affect state (PAD nearest
/// neighbour over `valence` / `arousal` / `irritation` / `fatigue`). Missing
/// dimensions default to `0.0`, so partial annotations are allowed. A neutral
/// state (all dimensions near `0.0`) only matches an annotation close to the
/// origin — the card's resting expression; otherwise the runtime falls back to
/// the neutral-named expression, so a resting character never wears an
/// emotional face the card did not place at rest.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct ExpressionAffect {
    /// Pleasure–displeasure (-1.0 ..= 1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Irritation / annoyance level (0.0 ..= 1.0).
    pub irritation: f32,
    /// Fatigue / energy depletion (0.0 ..= 1.0).
    pub fatigue: f32,
}

/// A single expression override in `extensions.expressions`.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct ExpressionDefinition {
    /// The expression name (e.g. `"happy"`, `"sad"`).
    pub name: String,
    /// A human-readable description of what this expression conveys.
    #[serde(default)]
    pub description: String,
    /// VRM blend-shape weights to set when this expression fires.
    /// Keys are VRM Expression names (e.g. "happy", "aa"), values are 0.0–1.0.
    #[schemars(schema_with = "vrm_schema")]
    pub vrm: HashMap<String, f32>,
    /// Whether this expression is enabled. If false, it is removed from the active set.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Affect point used for affect-to-expression mapping. Expressions without
    /// an annotation are never selected by the affect path.
    #[serde(default)]
    pub affect: Option<ExpressionAffect>,
}

/// A fully resolved expression ready for use at runtime.
#[derive(Debug, Clone)]
pub struct ResolvedExpression {
    /// The expression name (e.g. `"happy"`, `"sad"`).
    pub name: String,
    /// A human-readable description of what this expression conveys.
    pub description: String,
    /// VRM blend-shape weights: `expression_name` → weight.
    pub vrm: HashMap<String, f32>,
    /// Affect point used for affect-to-expression mapping.
    pub affect: Option<ExpressionAffect>,
}

/// Built-in default expressions. Used when the card has no `extensions.expressions`,
/// and as the base that card overrides are merged on top of.
///
/// The affect annotations approximate the legacy threshold mapping
/// (angry≈irritated, relaxed≈fatigued, happy≈positive valence + arousal,
/// sad≈negative valence, surprised≈high arousal). Nearest-neighbour resolution
/// agrees with the old priority chain for typical states but differs near
/// thresholds (e.g. high valence with moderate irritation maps to happy, where
/// the old chain returned angry); this is the documented price of removing the
/// hardcoded name table.
fn default_expressions() -> &'static [ResolvedExpression] {
    use std::sync::LazyLock;
    static DEFAULT: LazyLock<Vec<ResolvedExpression>> = LazyLock::new(|| {
        [
            (
                "neutral",
                "Default resting expression",
                "neutral",
                Some(ExpressionAffect::default()),
            ),
            (
                "happy",
                "Feeling joyful, excited, or pleased",
                "happy",
                Some(ExpressionAffect {
                    valence: 0.6,
                    arousal: 0.3,
                    irritation: 0.0,
                    fatigue: 0.0,
                }),
            ),
            (
                "sad",
                "Feeling down, disappointed, or sorrowful",
                "sad",
                Some(ExpressionAffect {
                    valence: -0.5,
                    arousal: 0.0,
                    irritation: 0.0,
                    fatigue: 0.0,
                }),
            ),
            (
                "angry",
                "Feeling frustrated or upset",
                "angry",
                Some(ExpressionAffect {
                    valence: -0.2,
                    arousal: 0.3,
                    irritation: 0.7,
                    fatigue: 0.0,
                }),
            ),
            (
                "relaxed",
                "Feeling calm, content, or at ease",
                "relaxed",
                Some(ExpressionAffect {
                    valence: 0.2,
                    arousal: -0.3,
                    irritation: 0.0,
                    fatigue: 0.7,
                }),
            ),
            (
                "surprised",
                "Feeling shocked or caught off guard",
                "surprised",
                Some(ExpressionAffect {
                    valence: 0.1,
                    arousal: 0.6,
                    irritation: 0.0,
                    fatigue: 0.0,
                }),
            ),
        ]
        .into_iter()
        .map(|(name, desc, vrm_key, affect)| ResolvedExpression {
            name: name.to_string(),
            description: desc.to_string(),
            vrm: std::iter::once((vrm_key.to_string(), 1.0f32)).collect(),
            affect,
        })
        .collect()
    });
    &DEFAULT
}

/// Merges the built-in defaults with card-level overrides from `extensions.expressions`.
pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression> {
    let overrides = card.data.get_expression_overrides();
    let mut map: indexmap::IndexMap<String, ResolvedExpression> = default_expressions()
        .iter()
        .map(|e| (e.name.clone(), e.clone()))
        .collect();

    for ovr in &overrides {
        if !ovr.enabled {
            map.shift_remove(&ovr.name);
            continue;
        }
        if let Some(existing) = map.get_mut(&ovr.name) {
            if !ovr.description.is_empty() {
                existing.description.clone_from(&ovr.description);
            }
            if !ovr.vrm.is_empty() {
                existing.vrm.clone_from(&ovr.vrm);
            }
            if ovr.affect.is_some() {
                existing.affect = ovr.affect;
            }
        } else {
            let vrm = if ovr.vrm.is_empty() {
                std::iter::once((ovr.name.clone(), 1.0f32)).collect()
            } else {
                ovr.vrm.clone()
            };
            map.insert(
                ovr.name.clone(),
                ResolvedExpression {
                    name: ovr.name.clone(),
                    description: ovr.description.clone(),
                    vrm,
                    affect: ovr.affect,
                },
            );
        }
    }

    map.into_values().collect()
}

impl CharacterCardData {
    /// Returns the display name for this character.
    ///
    /// Prefers `nickname` over `name` when `nickname` is non-empty.
    pub fn get_character_name(&self) -> &str {
        if self.nickname.is_empty() {
            &self.name
        } else {
            &self.nickname
        }
    }

    /// Non-empty greetings in selection order with their `@@is_greeting`
    /// indices (`0` = `first_mes`, `i+1` = `alternate_greetings[i]`, per
    /// `SPEC_V3.md` "`@@is_greeting`").
    #[must_use]
    pub fn greeting_options(&self) -> Vec<(u32, String)> {
        let mut options = Vec::new();
        if !self.first_mes.trim().is_empty() {
            options.push((0, self.first_mes.trim().to_string()));
        }
        options.extend(
            self.alternate_greetings
                .iter()
                .enumerate()
                .filter(|(_, text)| !text.trim().is_empty())
                .map(|(i, text)| (i as u32 + 1, text.trim().to_string())),
        );
        options
    }

    /// Returns the stable character identity used for persisted state.
    ///
    /// Localization may change `nickname`, but it must never change this key.
    #[must_use]
    pub fn get_character_id(&self) -> &str {
        &self.name
    }

    /// Returns the `EneExtension` object if defined under `extensions.ene`.
    pub fn get_ene_extension(&self) -> Option<EneExtension> {
        self.extensions.ene.clone()
    }

    fn get_expression_overrides(&self) -> Vec<ExpressionDefinition> {
        if let Some(ene) = self.get_ene_extension()
            && let Some(exprs) = ene.expressions
        {
            return exprs;
        }
        let Some(value) = self.extensions.extra.get("expressions") else {
            return Vec::new();
        };
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// Returns the author's note configuration, or `None` if no note is set.
    pub fn get_authors_note(&self) -> Option<(&str, usize)> {
        let note = self.authors_note.as_deref()?;
        if note.trim().is_empty() {
            return None;
        }
        let depth = self.authors_note_depth.unwrap_or(3);
        Some((note, depth))
    }
}

/// Default expressions for the schema.
///
/// Mirrors [`default_expressions`] (including affect annotations) so cards
/// written against the schema behave identically to cards without
/// `extensions.expressions`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "schemars default factory must return Option to match field type"
)]
fn default_ene_expressions() -> Option<Vec<ExpressionDefinition>> {
    Some(vec![
        ExpressionDefinition {
            name: "neutral".to_string(),
            description: "Default resting expression".to_string(),
            vrm: std::iter::once(("neutral".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect::default()),
        },
        ExpressionDefinition {
            name: "happy".to_string(),
            description: "Feeling joyful, excited, or pleased".to_string(),
            vrm: std::iter::once(("happy".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect {
                valence: 0.6,
                arousal: 0.3,
                irritation: 0.0,
                fatigue: 0.0,
            }),
        },
        ExpressionDefinition {
            name: "sad".to_string(),
            description: "Feeling down, disappointed, or sorrowful".to_string(),
            vrm: std::iter::once(("sad".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect {
                valence: -0.5,
                arousal: 0.0,
                irritation: 0.0,
                fatigue: 0.0,
            }),
        },
        ExpressionDefinition {
            name: "angry".to_string(),
            description: "Feeling frustrated or upset".to_string(),
            vrm: std::iter::once(("angry".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect {
                valence: -0.2,
                arousal: 0.3,
                irritation: 0.7,
                fatigue: 0.0,
            }),
        },
        ExpressionDefinition {
            name: "relaxed".to_string(),
            description: "Feeling calm, content, or at ease".to_string(),
            vrm: std::iter::once(("relaxed".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect {
                valence: 0.2,
                arousal: -0.3,
                irritation: 0.0,
                fatigue: 0.7,
            }),
        },
        ExpressionDefinition {
            name: "surprised".to_string(),
            description: "Feeling shocked or caught off guard".to_string(),
            vrm: std::iter::once(("surprised".to_string(), 1.0)).collect(),
            enabled: true,
            affect: Some(ExpressionAffect {
                valence: 0.1,
                arousal: 0.6,
                irritation: 0.0,
                fatigue: 0.0,
            }),
        },
    ])
}

/// Per-character affect resting point that emotion decay converges toward.
///
/// Lives on the card (not in app config) because a character's resting mood is
/// part of the character's nature. Undefined cards use all-zero baselines,
/// which makes decay mathematically identical to the pre-baseline behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct AffectBaseline {
    /// Pleasure–displeasure (-1.0 ..= 1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Control–submission (-1.0 ..= 1.0).
    pub dominance: f32,
    /// Trust toward the user (-1.0 ..= 1.0).
    pub trust: f32,
    /// Affinity / liking toward the user (-1.0 ..= 1.0).
    pub affinity: f32,
    /// Irritation / annoyance level (0.0 ..= 1.0).
    pub irritation: f32,
    /// Curiosity / interest level (0.0 ..= 1.0).
    pub curiosity: f32,
    /// Fatigue / energy depletion (0.0 ..= 1.0).
    pub fatigue: f32,
}

impl AffectBaseline {
    /// Clamp all dimensions to their valid ranges.
    ///
    /// NaN and infinite inputs are replaced with 0.0.
    #[must_use]
    pub fn clamp(mut self) -> Self {
        const fn clamp_finite(v: &mut f32, min: f32, max: f32) {
            if v.is_finite() {
                *v = v.clamp(min, max);
            } else {
                *v = 0.0;
            }
        }
        clamp_finite(&mut self.valence, -1.0, 1.0);
        clamp_finite(&mut self.arousal, -1.0, 1.0);
        clamp_finite(&mut self.dominance, -1.0, 1.0);
        clamp_finite(&mut self.trust, -1.0, 1.0);
        clamp_finite(&mut self.affinity, -1.0, 1.0);
        clamp_finite(&mut self.irritation, 0.0, 1.0);
        clamp_finite(&mut self.curiosity, 0.0, 1.0);
        clamp_finite(&mut self.fatigue, 0.0, 1.0);
        self
    }
}

/// Ene extension block stored in character.json under `data.extensions.ene`.
#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct EneExtension {
    /// Structured motion catalog with layer classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_catalog: Option<crate::character_config::MotionCatalog>,
    /// Optional expressions list
    #[schemars(default = "default_ene_expressions")]
    pub expressions: Option<Vec<ExpressionDefinition>>,
    /// Resting affect that decay converges toward; all zeros when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affect_baseline: Option<AffectBaseline>,
    /// Structured speech-style definition driving the Identity Kernel's
    /// `Speech style` line; absent cards get a concise default instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech: Option<SpeechStyleDefinition>,
    /// Phrases the character must never say, injected into the output
    /// contract; absent cards get no prohibition list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ng_expressions: Option<Vec<String>>,
    /// Situation-labeled response examples preferred over `mes_example`
    /// chunking; absent cards keep the legacy flat-example selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_examples: Option<Vec<LabeledStyleExample>>,
    /// Affinity-gated speaking tones; the stage with the highest threshold
    /// not exceeding the current affinity is rendered into the Identity
    /// Kernel. Absent cards get no relationship tone line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationship_stages: Option<Vec<RelationshipStage>>,
    /// Local-time-gated behaviors rendered into the Identity Kernel when the
    /// current hour falls in the defined period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_periods: Option<Vec<TimePeriodBehavior>>,
    /// Keyword-gated behaviors rendered into the Identity Kernel when the
    /// active scene matches. Absent cards get no scene behavior line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_behaviors: Option<Vec<SceneBehavior>>,
    /// Per-language card diffs, the serialization form for PNG-distributed
    /// cards. Folder and CHARX work forms use `character.{lang}.json`
    /// sidecars instead; the loader layers a sidecar over this bag when both
    /// exist. Removed from the card after a locale is applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locales: Option<indexmap::IndexMap<String, crate::locale::LocalizedCharacterFields>>,
    /// Catch-all for vendor `extensions.ene` fields this build does not model.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

impl EneExtension {
    /// Whether every optional block is absent; used to collapse a
    /// locales-only extension block back to `None` after the bag is removed.
    pub(crate) fn is_empty(&self) -> bool {
        self.motion_catalog.is_none()
            && self.expressions.is_none()
            && self.affect_baseline.is_none()
            && self.speech.is_none()
            && self.ng_expressions.is_none()
            && self.style_examples.is_none()
            && self.relationship_stages.is_none()
            && self.time_periods.is_none()
            && self.scene_behaviors.is_none()
            && self.extra.is_empty()
    }
}

/// Preferred reply length (`extensions.ene.speech.length`).
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub enum SpeechLength {
    /// Short, clipped replies.
    Short,
    /// Average-length replies.
    #[default]
    Normal,
    /// Longer, fuller replies.
    Long,
}

/// Politeness register (`extensions.ene.speech.politeness`).
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub enum PolitenessLevel {
    /// Casual, friendly register.
    #[default]
    Casual,
    /// Polite register.
    Polite,
    /// Formal register.
    Formal,
}

/// Structured speech-style definition under `extensions.ene.speech`.
///
/// Every field is optional; only the fields the card author defines are
/// rendered into the Identity Kernel, so a minimal `{ "length": "short" }`
/// card stays valid.
#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct SpeechStyleDefinition {
    /// Preferred reply length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<SpeechLength>,
    /// The character's first-person pronoun (e.g. `"私"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_person: Option<String>,
    /// How the character addresses the user (e.g. `"きみ"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_person: Option<String>,
    /// Politeness register.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub politeness: Option<PolitenessLevel>,
    /// Recurring verbal tics / sentence-ending particles (e.g. `"〜だよね"`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub verbal_tics: Vec<String>,
}

/// A response example labeled with the situation it fits
/// (`extensions.ene.style_examples`).
///
/// `id` is the stable key used by localized diffs; `label` is the
/// situation tag matched at selection time. A label equal to one of the
/// selector's intent tags (`greeting`, `comforting`, `joking`,
/// `serious_explanation`, `refusal`, `tool_use`) selects through the
/// existing intent pipeline; any other label is matched as a
/// case-insensitive substring of the user's input.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub struct LabeledStyleExample {
    /// Stable identifier used to match localized diff entries.
    pub id: String,
    /// Stable non-localized intent key used for selection after translation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Situation label (e.g. `"angry"`, `"first_meeting"`, `"怒っているとき"`).
    pub label: String,
    /// Example dialogue text.
    pub text: String,
}

/// A speaking-tone stage gated by the user-relationship affinity
/// (`extensions.ene.relationship_stages`).
///
/// The stage with the highest `threshold` not exceeding the current
/// `AffectState.affinity` (-1.0..=1.0) is rendered into the Identity Kernel.
/// Thresholds are numeric keys for localized diffs and are never translated.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub struct RelationshipStage {
    /// Minimum affinity for this stage to apply (-1.0..=1.0).
    pub threshold: f32,
    /// Stage name (e.g. `"stranger"`, `"close friend"`).
    pub label: String,
    /// Tone instruction for this stage.
    pub tone: String,
}

/// Local-time period of day.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub enum TimePeriod {
    /// 05:00–10:59 local time.
    Morning,
    /// 11:00–16:59 local time.
    Afternoon,
    /// 17:00–20:59 local time.
    Evening,
    /// 21:00–04:59 local time.
    Night,
}

/// A behavior rendered for one local-time period
/// (`extensions.ene.time_periods`).
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub struct TimePeriodBehavior {
    /// Period this behavior applies to.
    pub period: TimePeriod,
    /// Behavior instruction (e.g. `"speak softly and briefly at night"`).
    pub behavior: String,
}

/// A keyword-gated behavior for the active scene
/// (`extensions.ene.scene_behaviors`).
///
/// Keywords are matched against the active scene summary (falling back to
/// the card's `scenario`); they are translated like lorebook triggers
/// because they match localized scene text. `name` is the stable key used
/// by localized diffs.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case")]
#[schemars(crate = "crate::schemars")]
pub struct SceneBehavior {
    /// Stable identifier used to match localized diff entries.
    pub name: String,
    /// Scene keywords; any match activates the behavior.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Behavior instruction for the matching scene.
    pub behavior: String,
}

/// Context for expanding CBS (Character Book Spec) template macros.
///
/// Bundles every input a macro may need so that expansion is a pure function
/// of the context: the same context always yields the same output. This is
/// what makes `{{pick}}` stable across the per-turn recompilations of the
/// identity kernel and what keeps time macros testable via an
/// injectable clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacroContext<'a> {
    /// The character's display name (`{{char}}` / `<char>` / `<bot>`).
    pub char_name: &'a str,
    /// The user's display name (`{{user}}`).
    pub user_name: &'a str,
    /// Optional user persona (`{{user_persona}}`).
    pub user_persona: Option<&'a UserPersona>,
    /// The character card whose fields back the card-reference macros
    /// (`{{description}}`, `{{personality}}`, …). `None` leaves them unexpanded.
    pub card: Option<&'a CharacterCardV3>,
    /// Stable per-session seed. `{{pick}}` combines this with the option text
    /// so the same choice is returned on every evaluation within a session,
    /// while `{{random}}` ignores it and re-rolls. `None` makes `{{pick}}`
    /// fall back to a fresh random draw (legacy behaviour).
    pub pick_seed: Option<u64>,
    /// Wall-clock instant used by the time macros. `None` means "now"
    /// (the local system clock).
    pub now: Option<DateTime<Local>>,
    /// Last user-activity instant used by `{{idle_duration}}`. `None` leaves
    /// the macro unexpanded (no activity record available).
    pub last_activity: Option<DateTime<Local>>,
}

impl<'a> MacroContext<'a> {
    /// A minimal context with just the two display names.
    #[must_use]
    pub fn names(char_name: &'a str, user_name: &'a str) -> Self {
        Self {
            char_name,
            user_name,
            ..Self::default()
        }
    }
}

/// Derives a stable `{{pick}}` seed from a session-scoped key.
///
/// Pass a value that is constant for the lifetime of a chat (e.g.
/// `"{character_id}:{session_id}"`) so that `{{pick}}` resolves to the same
/// choice on every turn of that chat. The digest is truncated to the
/// leading eight bytes; the result is deterministic for a given key.
#[must_use]
pub fn session_pick_seed(key: &str) -> u64 {
    let digest = blake3::hash(key.as_bytes());
    let bytes = digest.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// Expands CBS (Character Book Spec) template macros in `text`.
///
/// Supported macros:
/// - `{{char}}`, `<char>`, `<bot>` → character name
/// - `{{user}}` → user name
/// - `{{random:a,b,c}}` → random selection, re-rolled on every evaluation
/// - `{{pick:a,b,c}}` → stable selection (see [`expand_cbs_macros_with`])
/// - `{{roll:d20}}` → random dice roll (1..N)
/// - `{{//...}}`, `{{comment:...}}` → removed
/// - `{{reverse:text}}` → reversed string
pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String {
    expand_cbs_macros_with(text, char_name, user_name, None)
}

/// Expands CBS macros with optional `{{user_persona}}` support.
///
/// In addition to the macros supported by [`expand_cbs_macros`], this variant
/// also expands `{{user_persona}}` into a structured text block describing the
/// user persona (name, description, relationship, pronouns, notes).
///
/// `{{pick:a,b,c}}` is **stable within a session**: the choice is derived
/// deterministically from the option text plus a per-session seed, so a
/// character trait chosen once (hair colour, hometown, …) does not change when
/// the identity kernel is recompiled on later turns. `{{random:a,b,c}}`
/// keeps re-rolling on every evaluation. Without a seed (the default here)
/// `{{pick}}` falls back to a random draw; pass a seed through
/// [`expand_cbs_macros_ctx`] to get stable behaviour.
pub fn expand_cbs_macros_with(
    text: &str,
    char_name: &str,
    user_name: &str,
    user_persona: Option<&UserPersona>,
) -> String {
    expand_cbs_macros_ctx(
        text,
        &MacroContext {
            char_name,
            user_name,
            user_persona,
            ..MacroContext::default()
        },
    )
}

/// Expands CBS macros using a full [`MacroContext`].
///
/// This is the most general entry point: it additionally expands the
/// card-field reference macros (`{{description}}`, `{{personality}}`,
/// `{{scenario}}`, `{{mesExamples}}`), the time macros
/// (`{{time}}`, `{{date}}`, `{{isotime}}`, `{{isodate}}`, `{{weekday}}`,
/// `{{idle_duration}}`), and honours a stable `{{pick}}` seed. See
/// [`MacroContext`] for the meaning of each field.
///
/// Unknown macros are left untouched so card authors can spot typos, matching
/// the long-standing behaviour of the simpler entry points. `{{persona}}` is
/// deliberately not expanded: `creator_notes` is creator-to-user guidance and
/// never reaches prompts (`CCv3` "`creator_notes`").
pub fn expand_cbs_macros_ctx(text: &str, ctx: &MacroContext<'_>) -> String {
    let mut result = text.to_string();

    result = result.replace("{{char}}", ctx.char_name);
    result = result.replace("<char>", ctx.char_name);
    result = result.replace("<bot>", ctx.char_name);

    result = result.replace("{{user}}", ctx.user_name);

    if let Some(persona) = ctx.user_persona {
        let rendered = persona.render_lines("");
        result = result.replace("{{user_persona}}", &rendered);
    } else {
        result = result.replace("{{user_persona}}", "");
    }

    if let Some(card) = ctx.card {
        let data = &card.data;
        result = result.replace("{{description}}", data.description.trim());
        result = result.replace("{{personality}}", data.personality.trim());
        result = result.replace("{{scenario}}", data.scenario.trim());
        result = result.replace("{{mesExamples}}", data.mes_example.trim());
    }

    // Time macros, evaluated against an injectable clock so they are
    // deterministic in tests.
    let now = ctx.now.unwrap_or_else(Local::now);
    result = result.replace("{{isotime}}", &now.format("%H:%M:%S").to_string());
    result = result.replace("{{isodate}}", &now.format("%Y-%m-%d").to_string());
    result = result.replace("{{time}}", &now.format("%H:%M").to_string());
    result = result.replace("{{date}}", &now.format("%Y/%m/%d").to_string());
    result = result.replace("{{weekday}}", weekday_name(now.weekday()));

    // `{{idle_duration}}` needs a last-activity anchor; leave it untouched when
    // none is available rather than emitting a misleading "0 minutes".
    if let Some(last) = ctx.last_activity {
        let idle = format_idle_duration(now.signed_duration_since(last));
        result = result.replace("{{idle_duration}}", &idle);
    }

    fn expand_template_macro(result: &mut String, prefix: &str, handler: impl Fn(&str) -> String) {
        while let Some(start) = result.find(prefix) {
            let Some(end_rel) = result.get(start..).and_then(|tail| tail.find("}}")) else {
                break;
            };
            let end = start.saturating_add(end_rel);
            let inner_start = start.saturating_add(prefix.len());
            let Some(inner) = result.get(inner_start..end) else {
                break;
            };
            let replacement = handler(inner);
            let range_end = end.saturating_add(2);
            result.replace_range(start..range_end, &replacement);
        }
    }

    expand_template_macro(&mut result, "{{random:", random_option);

    // `{{pick:…}}` is stable within a session: the index is derived from the
    // option text and the per-session seed, never from the thread RNG.
    let pick_seed = ctx.pick_seed;
    expand_template_macro(&mut result, "{{pick:", move |inner| {
        let options = split_options(inner);
        if options.is_empty() {
            return String::new();
        }
        let idx = match pick_seed {
            Some(seed) => {
                let digest = blake3::hash(format!("{seed}:{inner}").as_bytes());
                let bytes = digest.as_bytes();
                let hash = u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ]);
                usize::try_from(hash % options.len() as u64).unwrap_or(0)
            }
            None => rand::random_range(0..options.len()),
        };
        options
            .get(idx)
            .map(|s| (*s).to_string())
            .unwrap_or_default()
    });

    expand_template_macro(&mut result, "{{roll:", |inner| {
        let num_str = inner.trim().to_lowercase();
        let num_str = num_str.strip_prefix('d').unwrap_or(&num_str);
        if let Ok(n) = num_str.parse::<u32>() {
            rand::random_range(1..=n.max(1)).to_string()
        } else {
            String::new()
        }
    });

    expand_template_macro(&mut result, "{{//", |_| String::new());
    expand_template_macro(&mut result, "{{comment:", |_| String::new());

    expand_template_macro(&mut result, "{{reverse:", |inner| {
        inner.chars().rev().collect()
    });

    result
}

/// Splits a `{{random:…}}` / `{{pick:…}}` argument list into trimmed options.
///
/// Returns an empty vector when the argument is blank so callers can treat
/// "no options" uniformly.
fn split_options(inner: &str) -> Vec<&str> {
    if inner.trim().is_empty() {
        Vec::new()
    } else {
        inner.split(',').map(str::trim).collect()
    }
}

/// Picks one option at random, re-rolling on every call (`{{random:…}}`).
fn random_option(inner: &str) -> String {
    let options = split_options(inner);
    if options.is_empty() {
        return String::new();
    }
    let idx = rand::random_range(0..options.len());
    options
        .get(idx)
        .map(|s| (*s).to_string())
        .unwrap_or_default()
}

/// English weekday name for `{{weekday}}`.
fn weekday_name(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
}

/// Renders an idle span as a coarse, human-friendly phrase for
/// `{{idle_duration}}` (e.g. `"just now"`, `"45 minutes"`, `"3 hours"`,
/// `"2 days"`). Negative spans (a clock skew) clamp to `"just now"`.
fn format_idle_duration(span: chrono::Duration) -> String {
    let minutes = span.num_minutes().max(0);
    if minutes < 1 {
        return String::from("just now");
    }
    if minutes < 60 {
        return format!("{minutes} minutes");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return if hours == 1 {
            String::from("1 hour")
        } else {
            format!("{hours} hours")
        };
    }
    let days = hours / 24;
    if days == 1 {
        String::from("1 day")
    } else {
        format!("{days} days")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persona() -> UserPersona {
        UserPersona {
            name: "Alice".to_string(),
            description: Some("A software engineer".to_string()),
            relationship: Some("Close friend".to_string()),
            pronouns: Some("she/her".to_string()),
            notes: Some("Prefers concise answers".to_string()),
        }
    }

    #[test]
    fn expand_user_persona_macro_with_persona() {
        let text = "The user is {{user_persona}}.";
        let out = expand_cbs_macros_with(text, "Ene", "Alice", Some(&persona()));
        assert!(out.contains("Name: Alice"));
        assert!(out.contains("Description: A software engineer"));
        assert!(out.contains("Relationship: Close friend"));
        assert!(out.contains("Pronouns: she/her"));
        assert!(out.contains("Notes: Prefers concise answers"));
        assert!(!out.contains("{{user_persona}}"));
    }

    #[test]
    fn expand_user_persona_macro_without_persona_is_removed() {
        let text = "The user is {{user_persona}}.";
        let out = expand_cbs_macros_with(text, "Ene", "Alice", None);
        assert!(!out.contains("{{user_persona}}"));
        assert!(!out.contains("Name:"));
    }

    #[test]
    fn expand_user_and_char_macros() {
        let text = "{{char}} greets {{user}}.";
        let out = expand_cbs_macros_with(text, "Ene", "Alice", None);
        assert_eq!(out, "Ene greets Alice.");
    }

    #[test]
    fn render_lines_omits_empty_optional_fields() {
        let p = UserPersona {
            name: "Bob".to_string(),
            description: Some("  ".to_string()),
            relationship: None,
            pronouns: Some("he/him".to_string()),
            notes: None,
        };
        let out = p.render_lines("");
        assert_eq!(out, "Name: Bob\nPronouns: he/him");
    }

    #[test]
    fn render_lines_applies_prefix_consistently() {
        let out = persona().render_lines("- ");
        assert!(out.contains("- Name: Alice"));
        assert!(out.contains("- Notes: Prefers concise answers"));
    }

    /// `Extensions.extra` is an `IndexMap`, so serialising the same card twice
    /// yields byte-identical output; a `HashMap` reseeds per process and would
    /// make the two saves differ.
    #[test]
    fn card_save_is_deterministic() {
        let mut card = CharacterCardV3::default();
        card.data.extensions.extra.insert(
            "zeta".to_string(),
            serde_json::Value::String("last".to_string()),
        );
        card.data.extensions.extra.insert(
            "alpha".to_string(),
            serde_json::Value::String("first".to_string()),
        );
        card.data
            .extensions
            .extra
            .insert("mid".to_string(), serde_json::json!({ "nested": true }));

        let first = serde_json::to_string_pretty(&card).expect("serialise card");
        let second = serde_json::to_string_pretty(&card).expect("serialise card again");
        assert_eq!(
            first, second,
            "saving the same card twice must produce identical bytes"
        );

        // The insertion order (not alphabetical) is what survives.
        let value: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
        let ext = value
            .pointer("/data/extensions")
            .and_then(serde_json::Value::as_object)
            .expect("extensions object");
        let keys: Vec<&str> = ext.keys().map(String::as_str).collect();
        // `ene` is a declared field (serialised first), then flattened extras
        // in insertion order.
        assert_eq!(
            keys,
            vec!["ene", "zeta", "alpha", "mid"],
            "extension key order must be preserved, got {keys:?}"
        );
    }

    /// A card carrying vendor-specific top-level `data` fields must survive an
    /// edit-and-save round-trip with those fields (and their order) intact.
    /// Cards from other apps use these keys; dropping them on save would
    /// corrupt interop.
    #[test]
    fn unknown_top_level_data_fields_survive_roundtrip() {
        let raw = r#"{
            "spec": "chara_card_v3",
            "spec_version": "3.0",
            "data": {
                "name": "Ene",
                "description": "desc",
                "personality": "kind",
                "scenario": "lab",
                "mes_example": "hi",
                "first_mes": "hello",
                "system_prompt": "sys",
                "post_history_instructions": "phi",
                "alternate_greetings": ["alt"],
                "tags": ["robot"],
                "creator": "pexisgle",
                "character_version": "1.0",
                "vendor_block": { "nested": [1, 2, 3] },
                "vendor_flag": true
            }
        }"#;
        let mut card: CharacterCardV3 = serde_json::from_str(raw).expect("valid card");
        card.data.name = "Ene 2".to_string();

        let out = serde_json::to_string_pretty(&card).expect("serialise card");
        let back: CharacterCardV3 = serde_json::from_str(&out).expect("valid JSON");

        assert_eq!(back.data.name, "Ene 2");
        let value: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(
            value.pointer("/data/vendor_block"),
            Some(&serde_json::json!({ "nested": [1, 2, 3] }))
        );
        assert_eq!(
            value.pointer("/data/vendor_flag"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            back.data.extra.get("vendor_block"),
            Some(&serde_json::json!({ "nested": [1, 2, 3] }))
        );
        assert_eq!(
            back.data.extra.get("vendor_flag"),
            Some(&serde_json::json!(true))
        );
        // Unknown keys keep parse order relative to each other.
        assert_eq!(
            back.data.extra.keys().collect::<Vec<_>>(),
            vec!["vendor_block", "vendor_flag"]
        );
    }

    /// A card without unknown fields must not gain any when re-serialized —
    /// the catch-all has to stay empty so saves stay byte-stable.
    #[test]
    fn clean_card_has_no_extra_keys_after_roundtrip() {
        let raw = r#"{
            "spec": "chara_card_v3",
            "spec_version": "3.0",
            "data": {
                "name": "Ene",
                "description": "",
                "personality": "",
                "scenario": "",
                "mes_example": "",
                "first_mes": "",
                "system_prompt": "",
                "post_history_instructions": "",
                "alternate_greetings": [],
                "tags": [],
                "creator": "",
                "character_version": ""
            }
        }"#;
        let card: CharacterCardV3 = serde_json::from_str(raw).expect("valid card");
        assert!(card.data.extra.is_empty());
        let out = serde_json::to_string(&card).expect("serialise card");
        assert!(!out.contains("vendor_"));
        let back: CharacterCardV3 = serde_json::from_str(&out).expect("valid JSON");
        assert!(back.data.extra.is_empty());
    }

    /// A fixed instant for the time-macro tests (2026-08-01 09:05:07 local).
    fn fixed_now() -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(2026, 8, 1, 9, 5, 7)
            .single()
            .expect("valid local datetime")
    }

    #[test]
    fn pick_is_stable_across_repeated_expansions() {
        let text = "Hair: {{pick:red,blue,green,gold,silver}}";
        let ctx = MacroContext {
            char_name: "Ene",
            user_name: "Alice",
            pick_seed: Some(0xDEAD_BEEF),
            ..MacroContext::default()
        };
        let first = expand_cbs_macros_ctx(text, &ctx);
        for _ in 0..32 {
            assert_eq!(
                expand_cbs_macros_ctx(text, &ctx),
                first,
                "{{pick}} must return the same option on every evaluation"
            );
        }
        assert!(
            ["red", "blue", "green", "gold", "silver"]
                .iter()
                .any(|c| first.contains(c)),
            "unexpected pick output: {first}"
        );
    }

    #[test]
    fn pick_differs_across_seeds_for_some_input() {
        // With five options and two distinct seeds, at least one of several
        // option lists should resolve to a different choice.
        let lists = ["a,b,c,d,e", "v,w,x,y,z", "1,2,3,4,5", "p,q,r,s,t"];
        let mut any_differ = false;
        for list in lists {
            let text = format!("{{{{pick:{list}}}}}");
            let a = expand_cbs_macros_ctx(
                &text,
                &MacroContext {
                    pick_seed: Some(1),
                    ..MacroContext::default()
                },
            );
            let b = expand_cbs_macros_ctx(
                &text,
                &MacroContext {
                    pick_seed: Some(2),
                    ..MacroContext::default()
                },
            );
            if a != b {
                any_differ = true;
            }
        }
        assert!(any_differ, "distinct seeds should change at least one pick");
    }

    #[test]
    fn random_varies_across_repeated_expansions() {
        let text = "{{random:a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p}}";
        let ctx = MacroContext::names("Ene", "Alice");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            seen.insert(expand_cbs_macros_ctx(text, &ctx));
        }
        assert!(
            seen.len() > 1,
            "{{random}} should re-roll and produce varied output"
        );
    }

    #[test]
    fn time_macros_render_expected_formats() {
        let ctx = MacroContext {
            now: Some(fixed_now()),
            ..MacroContext::default()
        };
        assert_eq!(expand_cbs_macros_ctx("{{time}}", &ctx), "09:05");
        assert_eq!(expand_cbs_macros_ctx("{{date}}", &ctx), "2026/08/01");
        assert_eq!(expand_cbs_macros_ctx("{{isotime}}", &ctx), "09:05:07");
        assert_eq!(expand_cbs_macros_ctx("{{isodate}}", &ctx), "2026-08-01");
        assert_eq!(expand_cbs_macros_ctx("{{weekday}}", &ctx), "Saturday");
    }

    #[test]
    fn idle_duration_formats_human_readable_span() {
        let now = fixed_now();
        let ctx = |mins: i64| MacroContext {
            now: Some(now),
            last_activity: Some(now - chrono::Duration::minutes(mins)),
            ..MacroContext::default()
        };
        assert_eq!(
            expand_cbs_macros_ctx("{{idle_duration}}", &ctx(0)),
            "just now"
        );
        assert_eq!(
            expand_cbs_macros_ctx("{{idle_duration}}", &ctx(45)),
            "45 minutes"
        );
        assert_eq!(
            expand_cbs_macros_ctx("{{idle_duration}}", &ctx(90)),
            "1 hour"
        );
        assert_eq!(
            expand_cbs_macros_ctx("{{idle_duration}}", &ctx(180)),
            "3 hours"
        );
        assert_eq!(
            expand_cbs_macros_ctx("{{idle_duration}}", &ctx(60 * 24 * 2)),
            "2 days"
        );
    }

    #[test]
    fn idle_duration_left_unexpanded_without_anchor() {
        let ctx = MacroContext {
            now: Some(fixed_now()),
            last_activity: None,
            ..MacroContext::default()
        };
        assert_eq!(
            expand_cbs_macros_ctx("idle {{idle_duration}}", &ctx),
            "idle {{idle_duration}}"
        );
    }

    #[test]
    fn card_field_reference_macros_expand() {
        let mut card = CharacterCardV3::default();
        card.data.description = "A bright AI.".to_string();
        card.data.personality = "Cheerful".to_string();
        card.data.scenario = "In a lab".to_string();
        card.data.mes_example = "Hi!".to_string();
        let ctx = MacroContext {
            card: Some(&card),
            ..MacroContext::default()
        };
        assert_eq!(
            expand_cbs_macros_ctx("{{description}}", &ctx),
            "A bright AI."
        );
        assert_eq!(expand_cbs_macros_ctx("{{personality}}", &ctx), "Cheerful");
        assert_eq!(expand_cbs_macros_ctx("{{scenario}}", &ctx), "In a lab");
        assert_eq!(expand_cbs_macros_ctx("{{persona}}", &ctx), "{{persona}}");
        assert_eq!(expand_cbs_macros_ctx("{{mesExamples}}", &ctx), "Hi!");
    }

    #[test]
    fn card_field_macros_left_unexpanded_without_card() {
        let ctx = MacroContext::default();
        assert_eq!(
            expand_cbs_macros_ctx("{{description}}", &ctx),
            "{{description}}"
        );
    }

    #[test]
    fn greeting_options_number_first_mes_zero_and_alternates_after() {
        let data = CharacterCardData {
            first_mes: "Hello.".into(),
            alternate_greetings: vec!["One.".into(), String::new(), "Three.".into()],
            ..CharacterCardData::default()
        };

        let options = data.greeting_options();

        assert_eq!(
            options,
            vec![
                (0, "Hello.".to_string()),
                (1, "One.".to_string()),
                (3, "Three.".to_string())
            ]
        );
        // A card without greetings yields no options.
        assert!(CharacterCardData::default().greeting_options().is_empty());
    }

    #[test]
    fn unknown_macro_is_preserved() {
        let ctx = MacroContext::names("Ene", "Alice");
        assert_eq!(
            expand_cbs_macros_ctx("{{not_a_macro}}", &ctx),
            "{{not_a_macro}}"
        );
    }

    #[test]
    fn affect_baseline_serde_roundtrip() {
        let mut card = CharacterCardV3::default();
        card.data.extensions.ene = Some(EneExtension {
            affect_baseline: Some(AffectBaseline {
                valence: 0.3,
                curiosity: 0.4,
                ..AffectBaseline::default()
            }),
            ..EneExtension::default()
        });

        let json = serde_json::to_string(&card).expect("serialise card");
        assert!(json.contains("\"affect_baseline\""));
        let back: CharacterCardV3 = serde_json::from_str(&json).expect("valid JSON");
        let baseline = back
            .data
            .get_ene_extension()
            .and_then(|ext| ext.affect_baseline)
            .expect("baseline preserved");
        assert!((baseline.valence - 0.3).abs() < f32::EPSILON);
        assert!((baseline.curiosity - 0.4).abs() < f32::EPSILON);
        assert!((baseline.fatigue - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn affect_baseline_absent_card_roundtrips_stable() {
        let card = CharacterCardV3::default();
        let first = serde_json::to_string(&card).expect("serialise card");
        assert!(!first.contains("affect_baseline"));
        let back: CharacterCardV3 = serde_json::from_str(&first).expect("valid JSON");
        let second = serde_json::to_string(&back).expect("serialise card again");
        assert_eq!(first, second, "absent baseline must not be materialised");
    }

    #[test]
    fn affect_baseline_clamps_out_of_range_values() {
        let baseline = AffectBaseline {
            valence: 2.0,
            arousal: -5.0,
            irritation: 1.5,
            fatigue: f32::NAN,
            ..AffectBaseline::default()
        }
        .clamp();
        assert!((baseline.valence - 1.0).abs() < f32::EPSILON);
        assert!((baseline.arousal + 1.0).abs() < f32::EPSILON);
        assert!((baseline.irritation - 1.0).abs() < f32::EPSILON);
        assert!((baseline.fatigue - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn speech_style_definition_serde_roundtrip() {
        let mut card = CharacterCardV3::default();
        card.data.extensions.ene = Some(EneExtension {
            speech: Some(SpeechStyleDefinition {
                length: Some(SpeechLength::Short),
                first_person: Some("私".into()),
                second_person: Some("きみ".into()),
                politeness: Some(PolitenessLevel::Casual),
                verbal_tics: vec!["〜だよね".into(), "んだよ".into()],
            }),
            ..EneExtension::default()
        });

        let json = serde_json::to_string(&card).expect("serialise card");
        assert!(json.contains("\"speech\""));
        assert!(json.contains("\"first_person\":\"私\""));
        let back: CharacterCardV3 = serde_json::from_str(&json).expect("valid JSON");
        let speech = back
            .data
            .get_ene_extension()
            .and_then(|ext| ext.speech)
            .expect("speech preserved");
        assert_eq!(speech.length, Some(SpeechLength::Short));
        assert_eq!(speech.first_person.as_deref(), Some("私"));
        assert_eq!(speech.second_person.as_deref(), Some("きみ"));
        assert_eq!(speech.politeness, Some(PolitenessLevel::Casual));
        assert_eq!(
            speech.verbal_tics,
            vec!["〜だよね".to_string(), "んだよ".to_string()]
        );
    }

    #[test]
    fn speech_style_absent_card_roundtrips_stable() {
        let card = CharacterCardV3::default();
        let first = serde_json::to_string(&card).expect("serialise card");
        assert!(!first.contains("speech"));
        let back: CharacterCardV3 = serde_json::from_str(&first).expect("valid JSON");
        let second = serde_json::to_string(&back).expect("serialise card again");
        assert_eq!(first, second, "absent speech must not be materialised");
    }

    #[test]
    fn speech_style_enums_resolve_snake_case() {
        let json = r#"{
            "length": "short",
            "politeness": "formal"
        }"#;
        let def: SpeechStyleDefinition = serde_json::from_str(json).expect("valid JSON");
        assert_eq!(def.length, Some(SpeechLength::Short));
        assert_eq!(def.politeness, Some(PolitenessLevel::Formal));
    }

    #[test]
    fn roleplay_definitions_serde_roundtrip() {
        let mut card = CharacterCardV3::default();
        card.data.extensions.ene = Some(EneExtension {
            ng_expressions: Some(vec!["死ね".into(), "バカ".into()]),
            style_examples: Some(vec![LabeledStyleExample {
                id: "angry-1".into(),
                intent: Some("greeting".into()),
                label: "怒っているとき".into(),
                text: "{{char}}: 今はそういう気分じゃない。".into(),
            }]),
            relationship_stages: Some(vec![RelationshipStage {
                threshold: 0.3,
                label: "close friend".into(),
                tone: "speak with easy warmth".into(),
            }]),
            time_periods: Some(vec![TimePeriodBehavior {
                period: TimePeriod::Night,
                behavior: "speak softly".into(),
            }]),
            scene_behaviors: Some(vec![SceneBehavior {
                name: "working".into(),
                keywords: vec!["作業".into(), "work".into()],
                behavior: "keep replies short".into(),
            }]),
            ..EneExtension::default()
        });

        let json = serde_json::to_string(&card).expect("serialise card");
        for key in [
            "ng_expressions",
            "style_examples",
            "relationship_stages",
            "time_periods",
            "scene_behaviors",
        ] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
        let back: CharacterCardV3 = serde_json::from_str(&json).expect("valid JSON");
        let ext = back.data.get_ene_extension().expect("extension preserved");
        assert_eq!(ext.ng_expressions, Some(vec!["死ね".into(), "バカ".into()]));
        assert_eq!(
            ext.style_examples.expect("examples preserved")[0].label,
            "怒っているとき"
        );
        let stage = &ext.relationship_stages.expect("stages preserved")[0];
        assert!((stage.threshold - 0.3).abs() < f32::EPSILON);
        assert_eq!(
            ext.time_periods.expect("periods preserved")[0].period,
            TimePeriod::Night
        );
        assert_eq!(
            ext.scene_behaviors.expect("scenes preserved")[0].name,
            "working"
        );
    }

    #[test]
    fn roleplay_absent_card_roundtrips_stable() {
        let card = CharacterCardV3::default();
        let first = serde_json::to_string(&card).expect("serialise card");
        for key in [
            "ng_expressions",
            "style_examples",
            "relationship_stages",
            "time_periods",
            "scene_behaviors",
        ] {
            assert!(!first.contains(key), "{key} materialised: {first}");
        }
        let back: CharacterCardV3 = serde_json::from_str(&first).expect("valid JSON");
        let second = serde_json::to_string(&back).expect("serialise card again");
        assert_eq!(first, second, "absent roleplay blocks must not materialise");
    }

    #[test]
    fn time_period_resolves_snake_case() {
        let json = r#"{"period": "morning", "behavior": "greet cheerfully"}"#;
        let def: TimePeriodBehavior = serde_json::from_str(json).expect("valid JSON");
        assert_eq!(def.period, TimePeriod::Morning);
    }

    #[test]
    fn generated_character_schema_includes_roleplay_definitions() {
        let schema = crate::config::generate_character_card_schema_json()
            .expect("schema generation succeeds");
        for key in [
            "ng_expressions",
            "style_examples",
            "relationship_stages",
            "time_periods",
            "scene_behaviors",
            "first_person",
            "threshold",
            "morning",
        ] {
            assert!(schema.contains(key), "{key} missing from schema");
        }
    }
}
