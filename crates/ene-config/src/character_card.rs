use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A V3-format character card following the
/// [Character Card Spec](https://github.com/kwaroran/character-card-spec-v3).
pub struct CharacterCardV3 {
    /// Spec identifier (e.g. `"chara_card_v3"`).
    pub spec: String,
    /// Spec version (e.g. `"3.0"`).
    pub spec_version: String,
    /// The card's data payload.
    pub data: CharacterCardData,
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
    pub name: String,
    /// A short description of the character.
    pub description: String,
    /// Tags / categories for discovery.
    pub tags: Vec<String>,
    /// Who created this card.
    pub creator: String,
    /// Version string for this character definition.
    pub character_version: String,
    /// Example dialogue shown to the LLM on the first turn.
    pub mes_example: String,
    /// Extension key-value store (expressions, ene metadata, etc.).
    #[serde(default)]
    pub extensions: Extensions,
    /// The character's system prompt.
    pub system_prompt: String,
    /// Instructions appended after the conversation history (PHI).
    pub post_history_instructions: String,
    /// The character's opening message.
    pub first_mes: String,
    /// Alternate greeting messages that can replace `first_mes`.
    pub alternate_greetings: Vec<String>,
    /// Personality traits description.
    pub personality: String,
    /// Scenario / setting description.
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
    #[serde(default)]
    pub group_only_greetings: Vec<String>,
    /// Unix timestamp of when the card was created.
    #[serde(default)]
    pub creation_date: Option<u64>,
    /// Unix timestamp of the last modification.
    #[serde(default)]
    pub modification_date: Option<u64>,
}

/// Typed extension store for character cards.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(crate = "crate::serde")]
pub struct Extensions {
    /// Ene-specific extension block (motions, expressions, etc.).
    #[serde(default)]
    pub ene: Option<EneExtension>,
    /// Catch-all for other extension keys.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl schemars::JsonSchema for Extensions {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Extensions".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let ene_schema = generator.subschema_for::<Option<EneExtension>>();
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ene": ene_schema
            },
            "additionalProperties": true
        });
        serde_json::from_value(schema).unwrap_or_default()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
/// A reference to an external asset (VRM model, VRMA animation, etc.).
pub struct CharacterAsset {
    /// The type of asset (e.g. `"vrm"`, `"vrma"`, `"png"`).
    #[serde(rename = "type")]
    pub asset_type: String,
    /// URI pointing to the asset file.
    pub uri: String,
    /// Human-readable name for the asset.
    pub name: String,
    /// File extension (e.g. `"vrm"`, `"vrma"`).
    pub ext: String,
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
}

/// Built-in default expressions. Used when the card has no `extensions.expressions`,
/// and as the base that card overrides are merged on top of.
fn default_expressions() -> Vec<ResolvedExpression> {
    [
        ("neutral", "Default resting expression", "neutral"),
        ("happy", "Feeling joyful, excited, or pleased", "happy"),
        ("sad", "Feeling down, disappointed, or sorrowful", "sad"),
        ("angry", "Feeling frustrated or upset", "angry"),
        ("relaxed", "Feeling calm, content, or at ease", "relaxed"),
        (
            "surprised",
            "Feeling shocked or caught off guard",
            "surprised",
        ),
    ]
    .into_iter()
    .map(|(name, desc, vrm_key)| ResolvedExpression {
        name: name.to_string(),
        description: desc.to_string(),
        vrm: std::iter::once((vrm_key.to_string(), 1.0f32)).collect(),
    })
    .collect()
}

/// Merges the built-in defaults with card-level overrides from `extensions.expressions`.
#[must_use]
pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression> {
    let overrides = card.data.get_expression_overrides();
    let mut map: indexmap::IndexMap<String, ResolvedExpression> = default_expressions()
        .into_iter()
        .map(|e| (e.name.clone(), e))
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
    #[must_use]
    pub fn get_character_name(&self) -> &str {
        if self.nickname.is_empty() {
            &self.name
        } else {
            &self.nickname
        }
    }

    /// Returns the `EneExtension` object if defined under `extensions.ene`.
    #[must_use]
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
}

/// Default expressions for the schema.
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
        },
        ExpressionDefinition {
            name: "happy".to_string(),
            description: "Feeling joyful, excited, or pleased".to_string(),
            vrm: std::iter::once(("happy".to_string(), 1.0)).collect(),
            enabled: true,
        },
        ExpressionDefinition {
            name: "sad".to_string(),
            description: "Feeling down, disappointed, or sorrowful".to_string(),
            vrm: std::iter::once(("sad".to_string(), 1.0)).collect(),
            enabled: true,
        },
        ExpressionDefinition {
            name: "angry".to_string(),
            description: "Feeling frustrated or upset".to_string(),
            vrm: std::iter::once(("angry".to_string(), 1.0)).collect(),
            enabled: true,
        },
        ExpressionDefinition {
            name: "relaxed".to_string(),
            description: "Feeling calm, content, or at ease".to_string(),
            vrm: std::iter::once(("relaxed".to_string(), 1.0)).collect(),
            enabled: true,
        },
        ExpressionDefinition {
            name: "surprised".to_string(),
            description: "Feeling shocked or caught off guard".to_string(),
            vrm: std::iter::once(("surprised".to_string(), 1.0)).collect(),
            enabled: true,
        },
    ])
}

/// Ene extension block stored in character.json under `data.extensions.ene`.
#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde", rename_all = "snake_case", default)]
#[schemars(crate = "crate::schemars")]
pub struct EneExtension {
    /// Motions list (backward-compat; prefer `motion_catalog`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub motions: Vec<crate::character_config::MotionEntry>,
    /// Structured motion catalog with layer classification (#130).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_catalog: Option<crate::character_config::MotionCatalog>,
    /// Optional expressions list
    #[schemars(default = "default_ene_expressions")]
    pub expressions: Option<Vec<ExpressionDefinition>>,
}

/// Expands CBS (Character Book Spec) template macros in `text`.
///
/// Supported macros:
/// - `{{char}}`, `<char>`, `<bot>` → `char_name`
/// - `{{user}}` → `user_name`
/// - `{{random:a,b,c}}`, `{{pick:a,b,c}}` → random selection
/// - `{{roll:d20}}` → random dice roll (1..N)
/// - `{{//...}}`, `{{comment:...}}` → removed
/// - `{{reverse:text}}` → reversed string
#[must_use]
pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String {
    let mut result = text.to_string();

    result = result.replace("{{char}}", char_name);
    result = result.replace("<char>", char_name);
    result = result.replace("<bot>", char_name);

    result = result.replace("{{user}}", user_name);

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

    let pick_handler = |inner: &str| -> String {
        let options: Vec<&str> = inner.split(',').collect();
        if options.is_empty() || inner.trim().is_empty() {
            String::new()
        } else {
            let idx = rand::random_range(0..options.len());
            options
                .get(idx)
                .map(|s| (*s).to_string())
                .unwrap_or_default()
        }
    };

    expand_template_macro(&mut result, "{{random:", pick_handler);
    expand_template_macro(&mut result, "{{pick:", pick_handler);

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
