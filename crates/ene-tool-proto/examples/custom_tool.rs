//! Custom tool example using `ene-tool-proto` v2 + `#[derive(ToolSpec)]`.
//!
//! Demonstrates the full pattern for implementing a custom `ToolProvider`
//! that exposes multiple individual tools using the structured `ToolSpec` API
//! and the `#[derive(ToolSpec)]` proc-macro from `ene-tool-derive`.
//!
//! Each operation (add, subtract, multiply, divide) is its own first-class
//! tool with a typed args struct, auto-generated JSON Schema, and rich
//! metadata for the Tool RAG embedding pipeline.
//!
//! # Usage
//!
//! Build and place the binary in the tool directory, then enable in
//! `settings.json`:
//!
//! ```json
//! { "tools": { "tools": { "calculator": { "enable": true } } } }
//! ```
//!
//! Or run directly to test the provider logic:
//!
//! ```sh
//! cargo run -p ene-tool-proto --example custom_tool
//! ```

use async_trait::async_trait;
use ene_tool_derive::ToolSpec;
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec, run_tool_server};
use schemars::JsonSchema;
use serde::Deserialize;

// ──────────────────────────── Args structs ────────────────────────────

/// Args for the `calculator.add` tool.
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "calculator",
    name = "add",
    summary = "Add two numbers together.",
    category = "Utility",
    keywords_primary = "add, sum, plus, arithmetic",
    keywords_secondary = "math, number",
    side_effects = "ReadOnly",
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5",
    caveats = "Supports IEEE 754 floating-point arithmetic.",
    related = "calculator.subtract, calculator.multiply, calculator.divide"
)]
pub struct AddArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}

/// Args for the `calculator.subtract` tool.
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "calculator",
    name = "subtract",
    summary = "Subtract the second number from the first.",
    category = "Utility",
    keywords_primary = "subtract, minus, difference",
    keywords_secondary = "math, number",
    side_effects = "ReadOnly",
    related = "calculator.add"
)]
pub struct SubtractArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}

/// Args for the `calculator.multiply` tool.
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "calculator",
    name = "multiply",
    summary = "Multiply two numbers.",
    category = "Utility",
    keywords_primary = "multiply, product, times",
    keywords_secondary = "math, number",
    side_effects = "ReadOnly",
    related = "calculator.divide"
)]
pub struct MultiplyArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}

/// Args for the `calculator.divide` tool.
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "calculator",
    name = "divide",
    summary = "Divide the first number by the second.",
    category = "Utility",
    keywords_primary = "divide, quotient, ratio",
    keywords_secondary = "math, number",
    side_effects = "ReadOnly",
    caveats = "Division by zero returns an error.",
    related = "calculator.multiply"
)]
pub struct DivideArgs {
    /// Numerator.
    pub a: f64,
    /// Denominator. Must not be zero.
    pub b: f64,
}

// ──────────────────────────── Provider ────────────────────────────────

/// A calculator tool provider that exposes four individual tools.
///
/// Each tool is a first-class `ToolSpec` with its own typed args struct,
/// auto-generated JSON Schema, and rich metadata for the Tool RAG pipeline.
struct CalculatorProvider;

impl CalculatorProvider {
    /// All specs this provider offers.
    const ALL_SPECS: fn() -> Vec<ToolSpec> = || {
        vec![
            AddArgs::spec(),
            SubtractArgs::spec(),
            MultiplyArgs::spec(),
            DivideArgs::spec(),
        ]
    };
}

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        (Self::ALL_SPECS)()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddArgs::TOOL_NAME => {
                let args: AddArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                Ok(format!("{} + {} = {}", args.a, args.b, args.a + args.b))
            }
            SubtractArgs::TOOL_NAME => {
                let args: SubtractArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                Ok(format!("{} - {} = {}", args.a, args.b, args.a - args.b))
            }
            MultiplyArgs::TOOL_NAME => {
                let args: MultiplyArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                Ok(format!("{} * {} = {}", args.a, args.b, args.a * args.b))
            }
            DivideArgs::TOOL_NAME => {
                let args: DivideArgs =
                    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                if args.b == 0.0 {
                    return Err(ToolError::ExecutionFailed {
                        message: "Division by zero".into(),
                    });
                }
                Ok(format!("{} / {} = {}", args.a, args.b, args.a / args.b))
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_call_context(&self, _ctx: &ene_tool_proto::CallContext) {}
}

// ──────────────────────────── Main ────────────────────────────────────

#[tokio::main]
async fn main() {
    println!("Starting calculator tool server...");
    println!(
        "Socket path: {}",
        std::env::var("ENE_TOOL_SOCKET").unwrap_or_else(|_| "not set (run via ene)".into())
    );

    // Print the specs for demonstration.
    let provider = CalculatorProvider;
    for spec in provider.list_specs() {
        println!("  {} — {}", spec.name.as_str(), spec.description,);
        println!(
            "    parameters: {}",
            serde_json::to_string_pretty(&spec.parameters).unwrap_or_default()
        );
    }

    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("Tool server error: {e}");
        std::process::exit(1);
    }
}
