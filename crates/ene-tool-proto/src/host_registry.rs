use crate::{CallContext, SandboxConfigData, ToolError, ToolName, ToolProvider, ToolSpec};
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

    /// Register a tool provider.
    ///
    /// # Errors
    /// Returns [`ToolError::DuplicateName`] when the provider exposes a
    /// name already registered by a previous provider (#135).
    pub fn try_add_provider(&mut self, provider: Box<dyn ToolProvider>) -> Result<(), ToolError> {
        let idx = self.providers.len();
        let mut pending = Vec::new();
        for spec in provider.list_specs() {
            // `spec.name` was built by the provider via
            // `ToolName::new` (compile-time-validated string
            // literal from the `#[tool]` macro), so the inner
            // string is guaranteed valid; access via `as_str`
            // is a borrow, not a clone, so the registry stays
            // O(1) on add.
            let name = spec.name.as_str().to_string();
            if self.tool_index.contains_key(&name) {
                return Err(ToolError::DuplicateName { tool_name: name });
            }
            pending.push(name);
        }
        for name in pending {
            self.tool_index.insert(name, idx);
        }
        self.providers.push(provider);
        Ok(())
    }

    /// Register a tool provider.
    ///
    /// # Panics
    /// Panics on name collision. Prefer [`try_add_provider`](Self::try_add_provider).
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>) {
        if let Err(e) = self.try_add_provider(provider) {
            panic!("HostRegistry::add_provider failed: {e}");
        }
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
                tool_name: name.as_str().to_string(),
            }),
        }
    }

    /// Broadcasts call context to all registered providers.
    pub fn set_call_context(&self, ctx: &CallContext) {
        for provider in &self.providers {
            provider.set_call_context(ctx);
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
        // IPC tool names come off the wire — they are
        // untrusted. Use `try_new` and return a typed
        // `InvalidName` error rather than panicking, so a
        // hostile or malformed tool binary cannot crash the
        // host with an `assert`.
        let n = ToolName::try_new(name).map_err(|e| ToolError::InvalidName { reason: e })?;
        self.call_tool(&n, arguments).await
    }

    fn set_call_context(&self, ctx: &CallContext) {
        for provider in &self.providers {
            provider.set_call_context(ctx);
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
    use crate::ToolName;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        name: String,
        call_ctx: Arc<Mutex<Option<CallContext>>>,
        sandbox: Arc<Mutex<Option<SandboxConfigData>>>,
    }

    impl MockProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                call_ctx: Arc::new(Mutex::new(None)),
                sandbox: Arc::new(Mutex::new(None)),
            }
        }

        fn spec(&self) -> ToolSpec {
            ToolSpec::new(
                ToolName::new(format!("tool_{}", self.name)),
                format!("Tool from {}", self.name),
                serde_json::json!({}),
            )
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

        fn set_call_context(&self, ctx: &CallContext) {
            *self.call_ctx.lock().unwrap() = Some(ctx.clone());
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

    #[test]
    fn host_registry_duplicate_name_is_hard_error() {
        let mut reg = HostRegistry::new();
        reg.try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap();
        let err = reg
            .try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap_err();
        assert!(matches!(
            err,
            ToolError::DuplicateName { tool_name } if tool_name == "tool_alpha"
        ));
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
    fn host_registry_set_call_context_broadcasts() {
        let mut reg = HostRegistry::new();
        let p1 = MockProvider::new("alpha");
        let ctx_ref = Arc::clone(&p1.call_ctx);
        reg.add_provider(Box::new(p1));
        let ctx = CallContext {
            conversation_id: "conv_xyz".into(),
            turn_id: "turn_1".into(),
        };
        reg.set_call_context(&ctx);
        assert_eq!(
            ctx_ref.lock().unwrap().as_ref().unwrap().conversation_id,
            "conv_xyz"
        );
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
            db_socket: None,
            db_auth_token: None,
        };
        reg.set_sandbox(&sandbox);
        assert_eq!(sandbox_ref.lock().unwrap().as_ref(), Some(&sandbox));
    }
}
