//! Integration tests for `#[derive(ToolAction)]` and `#[tool_action]`.
#![expect(
    clippy::expect_used,
    reason = "proc-macro integration tests use expect for schema assertions"
)]

use async_trait::async_trait;
use ene_plugin_proto::ToolError;
use ene_tool_macros::{ToolAction, ToolSpec, tool_action};
use ene_tool_sdk::ToolAction as _;
use schemars::JsonSchema;
use serde::Deserialize;

// ── Basic ToolAction derive ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "test",
    name = "echo",
    summary = "Echoes input.",
    category = "Utility",
    keywords_primary = "echo, repeat"
)]
pub struct EchoArgs {
    /// The text to echo.
    pub text: String,
}

impl EchoArgs {
    #[expect(clippy::unused_async, reason = "generated execute() awaits run()")]
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("echo: {}", self.text))
    }
}

#[tokio::test]
async fn tool_action_derive_execute_dispatches_to_run() {
    let action = EchoArgs {
        text: String::new(),
    };
    let result = action
        .execute(r#"{"text":"hello"}"#)
        .await
        .expect("execute should succeed");
    assert_eq!(result, "echo: hello");
}

#[tokio::test]
async fn tool_action_derive_execute_invalid_json() {
    let action = EchoArgs {
        text: String::new(),
    };
    let err = action.execute("not json").await.expect_err("should fail");
    assert!(matches!(err, ToolError::InvalidArguments { .. }));
}

#[test]
fn tool_action_derive_name_matches_spec() {
    let action = EchoArgs {
        text: String::new(),
    };
    assert_eq!(action.name(), "test.echo");
    assert_eq!(action.definition().name.as_str(), "test.echo");
}

#[test]
fn tool_action_derive_rag_profile() {
    let action = EchoArgs {
        text: String::new(),
    };
    let profile = action.rag_profile();
    assert_eq!(profile.name.as_str(), "test.echo");
    assert_eq!(profile.summary, "Echoes input.");
}

// ── ToolAction derive with skip fields ──────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "test",
    name = "stateful",
    summary = "Stateful action.",
    category = "Utility",
    keywords_primary = "state"
)]
pub struct StatefulArgs {
    #[tool(skip)]
    #[serde(skip, default)]
    pub session_id: String,
    /// The value to process.
    pub value: String,
}

impl StatefulArgs {
    #[expect(clippy::unused_async, reason = "generated execute() awaits run()")]
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("session={}, value={}", self.session_id, self.value))
    }
}

#[tokio::test]
async fn tool_action_skip_field_copied_from_self() {
    let action = StatefulArgs {
        session_id: "sess-42".to_string(),
        value: String::new(),
    };
    let result = action
        .execute(r#"{"value":"data"}"#)
        .await
        .expect("execute should succeed");
    assert_eq!(result, "session=sess-42, value=data");
}

#[test]
fn tool_action_skip_field_hidden_from_schema() {
    let spec = StatefulArgs::spec();
    let props = spec
        .parameters
        .as_object()
        .expect("parameters is object")
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("has properties");
    assert!(
        !props.contains_key("session_id"),
        "skip field must not appear in schema"
    );
    assert!(props.contains_key("value"));
}

// ── #[tool_action] attribute macro ──────────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolSpec)]
#[tool(
    namespace = "test",
    name = "greet",
    summary = "Greets someone.",
    category = "Utility",
    keywords_primary = "greet, hello"
)]
pub struct GreetArgs {
    /// Name to greet.
    pub name: String,
}

pub struct GreetAction;

#[tool_action(args = GreetArgs)]
#[async_trait]
impl ene_tool_sdk::ToolAction for GreetAction {
    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: GreetArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: e.to_string(),
            })?;
        Ok(format!("Hello, {}!", args.name))
    }
}

#[tokio::test]
async fn tool_action_attr_macro_execute() {
    let action = GreetAction;
    let result = action
        .execute(r#"{"name":"world"}"#)
        .await
        .expect("execute should succeed");
    assert_eq!(result, "Hello, world!");
}

#[test]
fn tool_action_attr_macro_name_and_definition() {
    let action = GreetAction;
    assert_eq!(action.name(), "test.greet");
    assert_eq!(action.definition().name.as_str(), "test.greet");
}

#[test]
fn tool_action_attr_macro_rag_profile() {
    let action = GreetAction;
    let profile = action.rag_profile();
    assert_eq!(profile.name.as_str(), "test.greet");
}

// ── Schema overrides: enum_values, default, min/max ─────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolSpec)]
#[tool(
    namespace = "test",
    name = "constrained",
    summary = "Constrained args.",
    category = "Utility",
    keywords_primary = "constrained"
)]
pub struct ConstrainedArgs {
    /// Direction to move.
    #[arg(enum_values = "left, right, up, down", default = "up")]
    pub direction: String,
    /// Number of steps.
    #[arg(minimum = 1, maximum = 100)]
    pub steps: u32,
}

#[test]
fn schema_enum_values_applied() {
    let spec = ConstrainedArgs::spec();
    let props = spec
        .parameters
        .as_object()
        .expect("parameters is object")
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("has properties");
    let dir = props
        .get("direction")
        .and_then(|v| v.as_object())
        .expect("direction property exists");
    let enum_vals = dir
        .get("enum")
        .and_then(|v| v.as_array())
        .expect("enum values present");
    assert_eq!(enum_vals.len(), 4);
    assert!(enum_vals.contains(&serde_json::json!("left")));
    assert!(enum_vals.contains(&serde_json::json!("down")));
}

#[test]
fn schema_default_applied() {
    let spec = ConstrainedArgs::spec();
    let props = spec
        .parameters
        .as_object()
        .expect("parameters is object")
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("has properties");
    let dir = props
        .get("direction")
        .and_then(|v| v.as_object())
        .expect("direction property exists");
    assert_eq!(dir.get("default"), Some(&serde_json::json!("up")));
}

#[test]
fn schema_min_max_applied() {
    let spec = ConstrainedArgs::spec();
    let props = spec
        .parameters
        .as_object()
        .expect("parameters is object")
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("has properties");
    let steps = props
        .get("steps")
        .and_then(|v| v.as_object())
        .expect("steps property exists");
    assert_eq!(steps.get("minimum"), Some(&serde_json::json!(1)));
    assert_eq!(steps.get("maximum"), Some(&serde_json::json!(100)));
}

// ── Hidden field via #[arg(hidden)] ─────────────────────────────────

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolSpec)]
#[tool(
    namespace = "test",
    name = "with_hidden",
    summary = "Has a hidden field.",
    category = "Utility",
    keywords_primary = "hidden"
)]
pub struct HiddenFieldArgs {
    /// Visible field.
    pub visible: String,
    /// Internal field.
    #[arg(hidden)]
    pub internal_state: String,
}

#[test]
fn hidden_field_removed_from_schema() {
    let spec = HiddenFieldArgs::spec();
    let props = spec
        .parameters
        .as_object()
        .expect("parameters is object")
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("has properties");
    assert!(props.contains_key("visible"));
    assert!(
        !props.contains_key("internal_state"),
        "hidden field must not appear in schema"
    );
}
