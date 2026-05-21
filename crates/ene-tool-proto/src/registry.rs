use crate::{SandboxConfigData, ToolDefinition, ToolError, ToolProvider};
use async_trait::async_trait;
use std::collections::HashMap;

/// 複数の ToolProvider を集約し、ツール名でディスパッチするレジストリ
///
/// ユーザーがカスタムツールバイナリで複数の Provider を束ねたい場合に使用できる。
/// 個別ツールバイナリでは通常1つの Provider だけで十分。
pub struct HostRegistry {
    providers: Vec<Box<dyn ToolProvider>>,
    tool_index: HashMap<String, usize>,
}

impl HostRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            tool_index: HashMap::new(),
        }
    }

    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>) {
        let idx = self.providers.len();
        for tool in provider.list_tools() {
            self.tool_index.entry(tool.name).or_insert(idx);
        }
        self.providers.push(provider);
    }

    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::with_capacity(self.tool_index.len());
        for provider in &self.providers {
            tools.extend(provider.list_tools());
        }
        tools
    }

    pub async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match self.tool_index.get(name) {
            Some(&idx) => self.providers[idx].call_tool(name, arguments).await,
            None => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    pub fn set_session_id(&self, session_id: &str) {
        for provider in &self.providers {
            provider.set_session_id(session_id);
        }
    }

    pub fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        for provider in &self.providers {
            provider.set_sandbox(sandbox);
        }
    }
}

#[async_trait]
impl ToolProvider for HostRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.list_tools()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.call_tool(name, arguments).await
    }

    fn set_session_id(&self, session_id: &str) {
        self.set_session_id(session_id);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.set_sandbox(sandbox);
    }
}