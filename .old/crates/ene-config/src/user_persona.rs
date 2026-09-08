use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "crate::serde")]
#[schemars(crate = "crate::schemars")]
pub struct UserPersona {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub relationship: Option<String>,
    #[serde(default)]
    pub pronouns: Option<String>,
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
}
