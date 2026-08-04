use ene_plugin::prelude::*;

/// Echoes the caller's text back verbatim.
///
/// Read-only and stateless, so it is eligible for bounded parallel
/// dispatch (see `ToolSpec::is_parallelizable`).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "__NAMESPACE__",
    name = "echo",
    summary = "Echo the provided text back verbatim.",
    description = "Returns the text argument unchanged. Useful for connectivity checks and simple round-trip tests.",
    category = "Utility",
    keywords_primary = "echo, roundtrip, ping",
    side_effects = "ReadOnly"
)]
pub struct EchoAction {
    /// The text to echo back.
    ///
    /// `#[arg(min_length = 1)]` constrains the generated JSON Schema
    /// only; the derive does not enforce constraints at runtime, so
    /// `run` validates explicitly.
    #[arg(min_length = 1, max_length = 4096)]
    text: String,
}

impl EchoAction {
    async fn run(&self) -> Result<String, ToolError> {
        if self.text.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "text must not be empty".to_string(),
            });
        }
        Ok(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_input() {
        let action = EchoAction::default();
        assert_eq!(action.name(), "__NAMESPACE__.echo");
        assert_eq!(
            action.execute(r#"{"text":"hello"}"#).await.unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn empty_text_rejected() {
        let action = EchoAction::default();
        let err = action.execute(r#"{"text":"  "}"#).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn malformed_json_rejected() {
        let action = EchoAction::default();
        let err = action.execute("not json").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn spec_has_expected_name_and_constraints() {
        let spec = EchoAction::spec();
        assert_eq!(spec.name.as_str(), "__NAMESPACE__.echo");
        let props = spec.parameters.get("properties").unwrap();
        let text = props.get("text").unwrap();
        assert_eq!(text.get("minLength").unwrap(), 1);
        assert_eq!(text.get("maxLength").unwrap(), 4096);
    }
}
