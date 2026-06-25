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
//! let header = lib.get("memory.summaries_header");
//! let rendered = lib.render("memory.facts_header", &[("user_name", "Alice")]);
//! ```

/// Loads and accesses prompt templates from a JSON locale file.
///
/// Template variables use `{variable_name}` syntax. Call [`render`](Self::render)
/// to substitute variables; call [`get`](Self::get) for literal string access.
#[derive(Debug, Clone)]
pub struct PromptLibrary {
    data: serde_json::Value,
    lang: String,
}

impl PromptLibrary {
    /// Loads the prompt library for the given language code (e.g. `"en"`).
    ///
    /// Falls back to the built-in English defaults if the file cannot be read or
    /// parsed. Logs a warning in that case.
    #[must_use]
    pub fn load(lang: &str) -> Self {
        let path = crate::paths::prompt_file_path(lang);
        match Self::load_from_file(&path) {
            Ok(lib) => lib,
            Err(e) => {
                tracing::warn!(
                    "[PromptLibrary] Failed to load prompts/{lang}.json ({e}); using built-in English defaults"
                );
                Self::built_in_english()
            }
        }
    }

    fn load_from_file(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let data: serde_json::Value = serde_json::from_str(&content)?;
        // Extract lang from filename stem
        let lang = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("en")
            .to_string();
        Ok(Self { data, lang })
    }

    /// Returns the built-in compile-time English defaults.
    ///
    /// These are the same strings shipped in `assets/prompts/en.json` but
    /// embedded at compile time as a fallback so the application works even
    /// when assets are missing (e.g. during unit tests or CI).
    #[must_use]
    pub fn built_in_english() -> Self {
        let data: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/prompts/en.json"
        )))
        .expect("built-in en.json is always valid");
        Self {
            data,
            lang: "en".to_string(),
        }
    }

    /// Returns the language code this library was loaded for.
    #[must_use]
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Looks up a dot-separated key (e.g. `"memory.summaries_header"`) and
    /// returns the string value, or an empty string if not found.
    #[must_use]
    pub fn get(&self, key: &str) -> &str {
        self.navigate(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    }

    /// Looks up `key` and substitutes `{variable}` placeholders with the
    /// provided `(name, value)` pairs.
    ///
    /// Unknown variables are left as-is.
    #[must_use]
    pub fn render(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let template = self.get(key);
        substitute(template, vars)
    }

    fn navigate(&self, key: &str) -> Option<&serde_json::Value> {
        let mut cur = &self.data;
        for segment in key.split('.') {
            cur = cur.get(segment)?;
        }
        Some(cur)
    }
}

/// Substitutes `{variable_name}` placeholders in `template` with the provided
/// `(name, value)` pairs. Unknown variables are left untouched.
#[must_use]
pub fn substitute(template: &str, vars: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in vars {
        result = result.replace(&format!("{{{}}}", name), value);
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
        assert!(!lib.get("system.mascot_context").is_empty());
        assert!(!lib.get("emotion.header").is_empty());
        assert!(!lib.get("summarizer.system").is_empty());
    }

    #[test]
    fn get_missing_key_returns_empty() {
        let lib = PromptLibrary::built_in_english();
        assert_eq!(lib.get("nonexistent.key"), "");
    }

    #[test]
    fn render_substitutes_vars_in_nested_key() {
        let lib = PromptLibrary::built_in_english();
        let rendered = lib.render("memory.facts_header", &[("user_name", "Alice")]);
        assert!(rendered.contains("Alice"), "rendered: {rendered}");
    }
}
