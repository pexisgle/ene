use super::definition::{ToolDefinition, ToolRegistry};
use async_trait::async_trait;

pub struct CompositeToolRegistry {
    registries: Vec<Box<dyn ToolRegistry>>,
}

impl CompositeToolRegistry {
    pub fn new(registries: Vec<Box<dyn ToolRegistry>>) -> Self {
        Self { registries }
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        for registry in &self.registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        for r in &self.registries {
            if r.list_tools().iter().any(|t| t.name == name) {
                return r.call_tool(name, arguments).await;
            }
        }

        Err(format!("Tool {} not found", name))
    }
}
