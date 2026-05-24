use serde::{Serialize, Deserialize};
use schemars::JsonSchema;
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct CharacterCardV3 {
    pub spec: String,
    pub spec_version: String,
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
pub struct CharacterCardData {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub creator: String,
    pub character_version: String,
    pub mes_example: String,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    pub system_prompt: String,
    pub post_history_instructions: String,
    pub first_mes: String,
    pub alternate_greetings: Vec<String>,
    pub personality: String,
    pub scenario: String,
    
    // Changes from CCv2
    #[serde(default)]
    pub creator_notes: String,
    #[serde(default)]
    pub character_book: Option<Lorebook>,

    // New fields in CCv3
    #[serde(default)]
    pub assets: Vec<CharacterAsset>,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub creator_notes_multilingual: Option<HashMap<String, String>>,
    #[serde(default)]
    pub source: Option<Vec<String>>,
    #[serde(default)]
    pub group_only_greetings: Vec<String>,
    #[serde(default)]
    pub creation_date: Option<u64>,
    #[serde(default)]
    pub modification_date: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct CharacterAsset {
    #[serde(rename = "type")]
    pub asset_type: String,
    pub uri: String,
    pub name: String,
    pub ext: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct Lorebook {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub scan_depth: Option<u32>,
    #[serde(default)]
    pub token_budget: Option<u32>,
    #[serde(default)]
    pub recursive_scanning: Option<bool>,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    pub entries: Vec<LorebookEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct LorebookEntry {
    pub keys: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub extensions: HashMap<String, serde_json::Value>,
    pub enabled: bool,
    pub insertion_order: i32,
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    pub use_regex: bool,
    #[serde(default)]
    pub constant: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub selective: Option<bool>,
    #[serde(default)]
    pub secondary_keys: Option<Vec<String>>,
    #[serde(default)]
    pub position: Option<String>,
}

/// A single expression override in `extensions.expressions`.
#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct ExpressionDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// VRM blend-shape weights to set when this expression fires.
    /// Keys are VRM Expression names (e.g. "happy", "aa"), values are 0.0–1.0.
    #[serde(default)]
    pub vrm: HashMap<String, f32>,
    /// If true, this expression is removed from the active set.
    #[serde(default)]
    pub disabled: bool,
}

/// A fully resolved expression ready for use at runtime.
#[derive(Debug, Clone)]
pub struct ResolvedExpression {
    pub name: String,
    pub description: String,
    /// VRM blend-shape weights: expression_name → weight.
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
        vrm: [(vrm_key.to_string(), 1.0f32)].into_iter().collect(),
    })
    .collect()
}

/// Merges the built-in defaults with card-level overrides from `extensions.expressions`.
pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression> {
    let overrides = card.data.get_expression_overrides();
    let mut map: indexmap::IndexMap<String, ResolvedExpression> = default_expressions()
        .into_iter()
        .map(|e| (e.name.clone(), e))
        .collect();

    for ovr in &overrides {
        if ovr.disabled {
            map.shift_remove(&ovr.name);
            continue;
        }
        if let Some(existing) = map.get_mut(&ovr.name) {
            if !ovr.description.is_empty() {
                existing.description = ovr.description.clone();
            }
            if !ovr.vrm.is_empty() {
                existing.vrm = ovr.vrm.clone();
            }
        } else {
            let vrm = if ovr.vrm.is_empty() {
                [(ovr.name.clone(), 1.0f32)].into_iter().collect()
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
    pub fn get_character_name(&self) -> &str {
        if self.nickname.is_empty() {
            &self.name
        } else {
            &self.nickname
        }
    }

    fn get_expression_overrides(&self) -> Vec<ExpressionDefinition> {
        let Some(value) = self.extensions.get("expressions") else {
            return Vec::new();
        };
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String {
    let mut result = text.to_string();

    result = result.replace("{{char}}", char_name);
    result = result.replace("<char>", char_name);
    result = result.replace("<bot>", char_name);

    result = result.replace("{{user}}", user_name);

    fn expand_template_macro(result: &mut String, prefix: &str, handler: impl Fn(&str) -> String) {
        while let Some(start) = result.find(prefix) {
            if let Some(end_rel) = result[start..].find("}}") {
                let end = start + end_rel;
                let inner = &result[start + prefix.len()..end];
                let replacement = handler(inner);
                result.replace_range(start..end + 2, &replacement);
            } else {
                break;
            }
        }
    }

    let pick_handler = |inner: &str| -> String {
        let options: Vec<&str> = inner.split(',').collect();
        if options.is_empty() || inner.trim().is_empty() {
            String::new()
        } else {
            let idx = rand::random_range(0..options.len());
            options[idx].to_string()
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
