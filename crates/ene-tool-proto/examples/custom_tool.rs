//! Custom tool example using ene-tool-proto.
//!
//! Demonstrates the full pattern for implementing a custom ToolProvider
//! and running an IPC tool server. This example registers a `calculator`
//! tool that performs basic arithmetic.
//!
//! To test: build as a binary and place it in the tool directory, then
//! enable it in settings.json:
//!
//! ```json
//! { "tools": { "tools": { "calculator": { "enable": true } } } }
//! ```

use async_trait::async_trait;
use ene_tool_proto::{ToolProvider, ToolDefinition, ToolCategory, ToolError, run_tool_server};

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "calculator".into(),
            description: "Performs basic arithmetic: addition, subtraction, multiplication, and division. Use the action parameter to specify the operation.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "subtract", "multiply", "divide"],
                        "description": "The arithmetic operation to perform"
                    },
                    "a": {
                        "type": "number",
                        "description": "First operand"
                    },
                    "b": {
                        "type": "number",
                        "description": "Second operand"
                    }
                },
                "required": ["action", "a", "b"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec![
                "math".into(), "calculate".into(), "arithmetic".into(),
                "add".into(), "subtract".into(), "multiply".into(), "divide".into(),
            ],
        }]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "calculator" => {
                let args: serde_json::Value = serde_json::from_str(arguments)
                    .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;

                let action = args["action"].as_str().unwrap_or("add");
                let a = args["a"].as_f64().ok_or_else(|| ToolError::InvalidArguments {
                    message: "Missing or invalid 'a' parameter".into(),
                })?;
                let b = args["b"].as_f64().ok_or_else(|| ToolError::InvalidArguments {
                    message: "Missing or invalid 'b' parameter".into(),
                })?;

                let result = match action {
                    "add" => a + b,
                    "subtract" => a - b,
                    "multiply" => a * b,
                    "divide" => {
                        if b == 0.0 {
                            return Err(ToolError::ExecutionFailed {
                                message: "Division by zero".into(),
                            });
                        }
                        a / b
                    }
                    _ => return Err(ToolError::InvalidArguments {
                        message: format!("Unknown action: {}", action),
                    }),
                };

                Ok(format!("{} {} {} = {}", a, action, b, result))
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}

#[tokio::main]
async fn main() {
    println!("Starting calculator tool server...");
    println!("Socket path: {}",
        std::env::var("ENE_TOOL_SOCKET").unwrap_or_else(|_| "not set (run via ene)".into()));

    if let Err(e) = run_tool_server(Box::new(CalculatorProvider)).await {
        eprintln!("Tool server error: {}", e);
        std::process::exit(1);
    }
}
