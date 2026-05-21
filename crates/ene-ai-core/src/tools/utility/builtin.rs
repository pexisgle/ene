use super::super::definition::{ToolDefinition, ToolRegistry};
use async_trait::async_trait;

#[derive(Default)]
pub struct BuiltinToolRegistry;

impl BuiltinToolRegistry {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolRegistry for BuiltinToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        // 全 utility ツールは IPC 版（ene-tools/utility）に移行済み
        Vec::new()
    }

    async fn call_tool(&self, name: &str, _arguments: &str) -> Result<String, String> {
        Err(format!(
            "Built-in tool '{}' is no longer available; use the IPC tool host instead",
            name
        ))
    }
}
