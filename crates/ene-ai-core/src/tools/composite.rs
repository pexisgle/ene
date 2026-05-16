use super::definition::{ToolDefinition, ToolRegistry};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct CompositeToolRegistry {
    registries: Vec<Box<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
}

impl CompositeToolRegistry {
    pub fn new(registries: Vec<Box<dyn ToolRegistry>>) -> Self {
        let mut tool_index = HashMap::with_capacity(registries.len() * 4);
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                tool_index.entry(tool.name).or_insert(idx);
            }
        }
        Self {
            registries,
            tool_index,
        }
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::with_capacity(self.tool_index.len());
        for registry in &self.registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        match self.tool_index.get(name) {
            Some(&idx) => self.registries[idx].call_tool(name, arguments).await,
            None => Err(format!("Tool {} not found", name)),
        }
    }
}
