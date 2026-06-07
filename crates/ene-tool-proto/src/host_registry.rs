use crate::{SandboxConfigData, ToolError, ToolName, ToolProvider, ToolSpec};
use async_trait::async_trait;
use std::collections::HashMap;

/// Registry that aggregates multiple `ToolProvider`s and dispatches by tool name.
///
/// Can be used when users want to bundle multiple providers in a custom tool binary.
/// A single provider is usually sufficient for standalone tool binaries.
#[derive(Default)]
pub struct HostRegistry {
    providers: Vec<Box<dyn ToolProvider>>,
    tool_index: HashMap<String, usize>,
}

impl HostRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool provider. First-wins on name conflicts.
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>) {
        let idx = self.providers.len();
        for spec in provider.list_specs() {
            self.tool_index.entry(spec.name.0).or_insert(idx);
        }
        self.providers.push(provider);
    }

    /// Returns all tool specs from all registered providers.
    #[must_use]
    pub fn list_specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::with_capacity(self.tool_index.len());
        for provider in &self.providers {
            specs.extend(provider.list_specs());
        }
        specs
    }

    /// Call a tool by name, dispatching to the provider that registered it.
    pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError> {
        match self.tool_index.get(name.as_str()) {
            Some(&idx) => {
                self.providers[idx]
                    .call_tool(name.as_str(), arguments)
                    .await
            }
            None => Err(ToolError::NotFound {
                tool_name: name.0.clone(),
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
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        let n = ToolName::new(name);
        self.call_tool(&n, arguments).await
    }

    fn set_session_id(&self, session_id: &str) {
        for provider in &self.providers {
            provider.set_session_id(session_id);
        }
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
    use crate::{KeywordSet, SideEffects, ToolCategory, ToolVersion};
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

        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: ToolName::new(format!("tool_{}", self.name)),
                version: ToolVersion::default(),
                display_name: format!("Tool {}", self.name),
                summary: format!("Tool from {}", self.name),
                description: format!("Tool from {}", self.name),
                category: ToolCategory::Utility,
                keywords: KeywordSet::default(),
                parameters: serde_json::json!({}),
                examples: vec![],
                caveats: vec![],
                side_effects: SideEffects::ReadOnly,
                preconditions: vec![],
                related: vec![],
            }
        }
    }

    #[async_trait]
    impl ToolProvider for MockProvider {
        fn list_specs(&self) -> Vec<ToolSpec> {
            vec![self.spec()]
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
        assert!(reg.list_specs().is_empty());
    }

    #[test]
    fn host_registry_add_provider() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        assert_eq!(reg.list_specs().len(), 1);
        assert_eq!(reg.list_specs()[0].name.as_str(), "tool_alpha");
    }

    #[test]
    fn host_registry_aggregates_multiple_providers() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        reg.add_provider(Box::new(MockProvider::new("beta")));
        let tools = reg.list_specs();
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"tool_alpha"));
        assert!(names.contains(&"tool_beta"));
    }

    #[tokio::test]
    async fn host_registry_call_tool_found() {
        let mut reg = HostRegistry::new();
        reg.add_provider(Box::new(MockProvider::new("alpha")));
        let result = reg
            .call_tool(&ToolName::new("tool_alpha"), "arg1")
            .await
            .unwrap();
        assert_eq!(result, "tool_alpha called with arg1");
    }

    #[tokio::test]
    async fn host_registry_call_tool_not_found() {
        let reg = HostRegistry::new();
        let err = reg
            .call_tool(&ToolName::new("nonexistent"), "arg")
            .await
            .unwrap_err();
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
