use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CharacterCardV3 {
    pub spec: String,
    pub spec_version: String,
    pub data: CharacterCardData,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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
    #[serde(default)]
    pub creator_notes: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub group_only_greetings: Vec<String>,
    #[serde(default)]
    pub creation_date: Option<u64>,
    #[serde(default)]
    pub modification_date: Option<u64>,
    #[serde(default)]
    pub assets: Vec<CharacterAsset>,
    // Omitting Lorebook fields for now, adding base struct later if needed
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CharacterAsset {
    #[serde(rename = "type")]
    pub asset_type: String,
    pub uri: String,
    pub name: String,
    pub ext: String,
}

/// A single expression override in `extensions.expressions`.
///
/// JSON shape:
/// ```json
/// { "name": "happy", "description": "Joyful", "vrm": {"happy": 1.0, "aa": 0.3} }
/// { "name": "think", "description": "Deep in thought", "vrm": {"neutral": 0.5} }
/// { "name": "relaxed", "disabled": true }
/// ```
#[derive(Debug, Serialize, Deserialize, Clone)]
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

/// Built-in default expressions.  Used when the card has no `extensions.expressions`,
/// and as the base that card overrides are merged on top of.
fn default_expressions() -> Vec<ResolvedExpression> {
    [
        ("neutral",   "Default resting expression",               "neutral"),
        ("happy",     "Feeling joyful, excited, or pleased",      "happy"),
        ("sad",       "Feeling down, disappointed, or sorrowful", "sad"),
        ("angry",     "Feeling frustrated or upset",              "angry"),
        ("relaxed",   "Feeling calm, content, or at ease",        "relaxed"),
        ("surprised", "Feeling shocked or caught off guard",      "surprised"),
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
///
/// Rules:
/// - Entries with `disabled: true` remove the expression from the set.
/// - Entries with a matching name override `description` (if non-empty) and `vrm` (if non-empty).
/// - Entries with a name not in the defaults are added as new expressions.
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
            // Override description if provided
            if !ovr.description.is_empty() {
                existing.description = ovr.description.clone();
            }
            // Override VRM weights if provided
            if !ovr.vrm.is_empty() {
                existing.vrm = ovr.vrm.clone();
            }
        } else {
            // New expression not in defaults
            let vrm = if ovr.vrm.is_empty() {
                // If no VRM specified, default to same-name expression at 1.0
                [(ovr.name.clone(), 1.0f32)].into_iter().collect()
            } else {
                ovr.vrm.clone()
            };
            map.insert(ovr.name.clone(), ResolvedExpression {
                name: ovr.name.clone(),
                description: ovr.description.clone(),
                vrm,
            });
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

    /// Returns raw expression overrides from `extensions.expressions`.
    fn get_expression_overrides(&self) -> Vec<ExpressionDefinition> {
        let Some(value) = self.extensions.get("expressions") else {
            return Vec::new();
        };
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}


pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String {
    let mut result = text.to_string();

    // {{char}} -> nickname or name
    result = result.replace("{{char}}", char_name);
    result = result.replace("<char>", char_name);
    result = result.replace("<bot>", char_name);

    // {{user}} -> user name
    result = result.replace("{{user}}", user_name);

    // Simple regex-less replacement for custom macros. 
    // For a robust implementation, we would use regex.
    
    // {{random:...}} or {{pick:...}}
    fn expand_template_macro(
        result: &mut String,
        prefix: &str,
        handler: impl Fn(&str) -> String,
    ) {
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
