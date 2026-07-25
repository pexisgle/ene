use async_trait::async_trait;
use ene_plugin_proto::{
    CallContext, SandboxConfigData, ToolError, ToolName, ToolProvider, ToolRagProfile, ToolSpec,
};
use std::collections::HashMap;

/// Registry that aggregates multiple `ToolProvider`s and dispatches by tool name.
///
/// Can be used when users want to bundle multiple providers in a custom tool binary.
/// A single provider is usually sufficient for standalone tool binaries.
#[derive(Default)]
pub struct HostRegistry {
    providers: Vec<Box<dyn ToolProvider>>,
    tool_index: HashMap<String, usize>,
    /// Cached specs from all registered providers, populated at
    /// `try_add_provider` time so `list_specs()` never re-queries
    /// the underlying providers.
    cached_specs: Vec<ToolSpec>,
}

impl HostRegistry {
    /// Creates an empty registry.
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
        let specs = provider.list_specs();
        let mut pending = Vec::new();
        for spec in &specs {
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
        self.cached_specs.extend(specs);
        self.providers.push(provider);
        Ok(())
    }

    /// Returns all tool specs from all registered providers.
    pub fn list_specs(&self) -> Vec<ToolSpec> {
        self.cached_specs.clone()
    }

    /// Returns all RAG profiles from all registered providers.
    pub fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        let mut profiles = Vec::new();
        for provider in &self.providers {
            profiles.extend(provider.list_rag_profiles());
        }
        profiles
    }

    /// Call a tool by name, dispatching to the provider that registered it.
    pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError> {
        match self.tool_index.get(name.as_str()) {
            Some(&idx) => match self.providers.get(idx) {
                Some(provider) => provider.call_tool(name.as_str(), arguments).await,
                None => Err(ToolError::NotFound {
                    tool_name: name.as_str().to_string(),
                }),
            },
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

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.list_rag_profiles()
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
        HostRegistry::set_call_context(self, ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        HostRegistry::set_sandbox(self, sandbox);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        reg.try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap();
        assert_eq!(reg.list_specs().len(), 1);
        assert_eq!(
            reg.list_specs().first().unwrap().name.as_str(),
            "tool_alpha"
        );
    }

    #[test]
    fn host_registry_aggregates_multiple_providers() {
        let mut reg = HostRegistry::new();
        reg.try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap();
        reg.try_add_provider(Box::new(MockProvider::new("beta")))
            .unwrap();
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
        reg.try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap();
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

    #[tokio::test]
    async fn host_registry_call_tool_invalid_name_via_trait() {
        let mut reg = HostRegistry::new();
        reg.try_add_provider(Box::new(MockProvider::new("alpha")))
            .unwrap();

        // Names with characters outside the valid set must produce a typed
        // `InvalidName` error rather than panicking (the trait path uses
        // `ToolName::try_new` for untrusted IPC input).
        let err = ToolProvider::call_tool(&reg, "bad/name", "")
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidName { .. }),
            "expected InvalidName, got: {err:?}"
        );

        // Consecutive separators are also invalid.
        let err = ToolProvider::call_tool(&reg, "a..b", "").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidName { .. }));

        // An empty name is invalid.
        let err = ToolProvider::call_tool(&reg, "", "").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidName { .. }));
    }

    #[test]
    fn host_registry_set_call_context_broadcasts() {
        let mut reg = HostRegistry::new();
        let p1 = MockProvider::new("alpha");
        let ctx_ref = Arc::clone(&p1.call_ctx);
        reg.try_add_provider(Box::new(p1)).unwrap();
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
        reg.try_add_provider(Box::new(p1)).unwrap();
        let sandbox = SandboxConfigData {
            enabled: true,
            allowed_directories: vec![],
            writable_directories: vec![],
            db_socket: None,
            db_auth_token: None,
            ..SandboxConfigData::default()
        };
        reg.set_sandbox(&sandbox);
        assert_eq!(sandbox_ref.lock().unwrap().as_ref(), Some(&sandbox));
    }
}
