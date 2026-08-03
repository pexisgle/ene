use ene_plugin::prelude::*;

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "filesystem",
    name = "regex.test",
    summary = "Test whether a regex pattern matches a string.",
    description = "Test whether a regex pattern matches a string, returning 'true' or 'false'. The pattern is matched anywhere in the string, with the same semantics as filesystem.grep.",
    category = "Filesystem",
    keywords_primary = "regex, test, match, pattern, validate, check",
    side_effects = "ReadOnly"
)]
pub struct FsRegexTestAction {
    /// String to test against the pattern.
    text: String,
    /// Regex pattern to match.
    pattern: String,
}

impl FsRegexTestAction {
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            pattern: String::new(),
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        if self.pattern.is_empty() {
            return Err(ToolError::execution_failed(
                "pattern is required".to_string(),
            ));
        }

        let re = regex::Regex::new(&self.pattern)
            .map_err(|e| ToolError::execution_failed(format!("Invalid regex pattern: {e}")))?;

        Ok(if re.is_match(&self.text) {
            "true".to_string()
        } else {
            "false".to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(text: &str, pattern: &str) -> FsRegexTestAction {
        FsRegexTestAction {
            text: text.to_string(),
            pattern: pattern.to_string(),
        }
    }

    #[tokio::test]
    async fn matching_pattern_returns_true() {
        assert_eq!(action("hello world", r"w\w+").run().await.unwrap(), "true");
    }

    #[tokio::test]
    async fn non_matching_pattern_returns_false() {
        assert_eq!(action("hello world", r"\d+").run().await.unwrap(), "false");
    }

    #[tokio::test]
    async fn matches_anywhere_in_the_string() {
        assert_eq!(
            action("prefix needle suffix", "needle")
                .run()
                .await
                .unwrap(),
            "true"
        );
    }

    #[tokio::test]
    async fn empty_pattern_is_an_error() {
        let err = action("anything", "").run().await.unwrap_err().to_string();
        assert!(err.contains("pattern is required"), "{err}");
    }

    #[tokio::test]
    async fn invalid_pattern_is_an_error() {
        let err = action("anything", "([")
            .run()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid regex pattern"), "{err}");
    }
}
