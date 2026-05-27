use crate::{SandboxConfigData, ToolDefinition, ToolError, ToolProvider};
use async_trait::async_trait;
use std::collections::HashMap;

/// Registry that aggregates multiple ToolProviders and dispatches by tool name
///
/// Can be used when users want to bundle multiple providers in a custom tool binary.
/// A single provider is usually sufficient for standalone tool binaries.
pub struct HostRegistry {
    providers: Vec<Box<dyn ToolProvider>>,
    tool_index: HashMap<String, usize>,
}

impl HostRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            tool_index: HashMap::new(),
        }
    }

    /// Register a tool provider. First-wins on name conflicts.
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>) {
        let idx = self.providers.len();
        for tool in provider.list_tools() {
            self.tool_index.entry(tool.name).or_insert(idx);
        }
        self.providers.push(provider);
    }

    /// Returns all tool definitions from all registered providers.
    pub fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::with_capacity(self.tool_index.len());
        for provider in &self.providers {
            tools.extend(provider.list_tools());
        }
        tools
    }

    /// Call a tool by name, dispatching to the provider that registered it.
    pub async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match self.tool_index.get(name) {
            Some(&idx) => self.providers[idx].call_tool(name, arguments).await,
            None => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    /// Broadcasts the session ID to all registered providers.
    pub fn set_session_id(&self, session_id: &str) {
        for provider in &self.providers {
            provider.set_session_id(session_id);
        }
    }

    /// Broadcasts sandbox configuration to all registered providers.
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
        for provider in &self.providers {
            provider.set_sandbox(sandbox);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCategory;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        name: String,
        session_id: Arc<Mutex<Option<String>>>,
        sandbox: Arc<Mutex<Option<SandboxConfigData>>>,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                session_id: Arc::new(Mutex::new(None)),
                sandbox: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl ToolProvider for MockProvider {
        fn list_tools(&self) -> Vec<ToolDefinition> {
            vec![ToolDefinition {
                name: format!("tool_{}", self.name),
                description: format!("Tool from {}", self.name),
                parameters: serde_json::json!({}),
                category: Some(ToolCategory::Utility),
                keywords: vec![],
            }]
        }

        async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
            Ok(format!("{name} called with {arguments}"))
        }

        fn set_session_id(&self, session_id: &str) {
            *self.session_id.lock().unwrap() = Some(session_id.to_string());
        }

        fn set_sandbox(&self, sandbox: &SandboxConfigData) {
            *self.sandbox.lock().unwrap() = Some(sandbox.clone());
        }
    }

    #[test]
    fn host_registry_new_is_empty() {
        let reg = HostRegistry::new();
        assert!(reg.list_tools().is_empty());
    }

    #[test]
    fn host_registry_add_provider() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        assert_eq!(reg.list_tools().len(), 1);
        assert_eq!(reg.list_tools()[0].name, "tool_alpha");
    }

    #[test]
    fn host_registry_aggregates_multiple_providers() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        reg.add_provider(Box::new(MockProvider::new("beta")));
        let tools = reg.list_tools();
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"tool_alpha"));
        assert!(names.contains(&"tool_beta"));
    }

    #[tokio::test]
    async fn host_registry_call_tool_found() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        let result = reg.call_tool("tool_alpha", "arg1").await.unwrap();
        assert_eq!(result, "tool_alpha called with arg1");
    }

    #[tokio::test]
    async fn host_registry_call_tool_not_found() {
        let reg = HostRegistry::new();
        let err = reg.call_tool("nonexistent", "arg").await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
        assert_eq!(format!("{err}"), "Tool not found: nonexistent");
    }

    #[test]
    fn host_registry_set_session_id_broadcasts() {
        let mut reg = HostRegistry::new();
        let p1 = MockProvider::new("alpha");
        let session_ref = Arc::clone(&p1.session_id);
        reg.add_provider(Box::new(p1));
        reg.set_session_id("sess_xyz");
        assert_eq!(session_ref.lock().unwrap().as_deref(), Some("sess_xyz"));
    }

    #[test]
    fn host_registry_set_sandbox_broadcasts() {
        let mut reg = HostRegistry::new();
        let p1 = MockProvider::new("alpha");
        let sandbox_ref = Arc::clone(&p1.sandbox);
        reg.add_provider(Box::new(p1));
        let sandbox = SandboxConfigData {
            enabled: true,
            allowed_directories: vec![],
            writable_directories: vec![],
            blocked_commands: vec![],
            max_read_bytes: 0,
            max_write_bytes: 0,
            shell_timeout_ms: 0,
            max_shell_output_bytes: 0,
            max_shell_output_lines: 0,
            undo_db_path: None,
        };
        reg.set_sandbox(&sandbox);
        assert_eq!(sandbox_ref.lock().unwrap().as_ref(), Some(&sandbox));
    }
}
