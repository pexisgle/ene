use super::registry::ToolRegistry;
use crate::ToolHostError;
use async_trait::async_trait;
use ene_tool_proto::{ToolRagProfile, ToolSpec};
use std::collections::HashMap;
use std::sync::Arc;

/// A tool registry that aggregates multiple sub-registries.
///
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag).
/// This registry only handles dispatch (list, call, config).
///
/// Name collision across sub-registries is a hard error — per API v2 / #135,
/// every tool must have a unique public name.
pub struct CompositeToolRegistry {
    state: std::sync::RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
}

impl CompositeToolRegistry {
    /// Creates a new composite tool registry from the given sub-registries.
    ///
    /// # Errors
    /// Returns [`ToolHostError::DuplicateToolName`] when two or more
    /// sub-registries expose a tool with the same name.
    pub fn try_new(registries: Vec<Arc<dyn ToolRegistry>>) -> Result<Self, ToolHostError> {
        let mut tool_index = HashMap::with_capacity(registries.len() * 4);
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if tool_index.contains_key(&name) {
                    return Err(ToolHostError::DuplicateToolName { tool_name: name });
                }
                tool_index.insert(name, idx);
            }
        }
        Ok(Self {
            state: std::sync::RwLock::new(CompositeState {
                registries,
                tool_index,
            }),
        })
    }

    /// Creates a new composite tool registry from the given sub-registries.
    ///
    /// # Panics
    /// Panics when two registries expose the same public tool name.
    /// Prefer [`try_new`](Self::try_new) at fallible call sites.
    #[must_use]
    pub fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self {
        match Self::try_new(registries) {
            Ok(composite) => composite,
            Err(ToolHostError::DuplicateToolName { tool_name }) => {
                panic!("Duplicate tool name in CompositeToolRegistry::new: {tool_name}");
            }
            Err(e) => panic!("CompositeToolRegistry::new failed: {e}"),
        }
    }

    /// Read-locks state and calls `f` with a reference to the registries slice.
    fn with_registries<R>(&self, f: impl FnOnce(&[Arc<dyn ToolRegistry>]) -> R) -> R {
        let guard = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&guard.registries)
    }

    /// Write-locks state and calls `f` with a mutable reference to `CompositeState`.
    fn with_state_mut<R>(&self, f: impl FnOnce(&mut CompositeState) -> R) -> R {
        let mut guard = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// Adds a sub-registry to the composite.
    ///
    /// # Errors
    /// Returns [`ToolHostError::DuplicateToolName`] when the new registry
    /// contains a tool name that already exists.
    pub fn try_add_registry(&self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError> {
        self.with_state_mut(|state| {
            let idx = state.registries.len();
            for tool in registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if state.tool_index.contains_key(&name) {
                    return Err(ToolHostError::DuplicateToolName { tool_name: name });
                }
                state.tool_index.insert(name, idx);
            }
            state.registries.push(registry);
            Ok(())
        })
    }

    /// Adds a sub-registry to the composite.
    ///
    /// # Panics
    /// Panics on name collision. Prefer [`try_add_registry`](Self::try_add_registry).
    pub fn add_registry(&self, registry: Arc<dyn ToolRegistry>) {
        match self.try_add_registry(registry) {
            Ok(()) => {}
            Err(e) => {
                tracing::error!(component = "CompositeToolRegistry", error = %e, "Failed to add registry");
                panic!("CompositeToolRegistry::add_registry failed: {e}");
            }
        }
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        let guard = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut tools = Vec::new();
        for registry in &guard.registries {
            tools.extend(registry.list_tools());
        }
        drop(guard);
        tools
    }

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        let guard = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut profiles = Vec::new();
        for registry in &guard.registries {
            profiles.extend(registry.list_rag_profiles());
        }
        drop(guard);
        profiles
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        let registry = {
            let guard = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(&idx) = guard.tool_index.get(name) else {
                return Err(ToolHostError::Protocol(
                    ene_tool_proto::ToolError::NotFound {
                        tool_name: name.to_string(),
                    },
                ));
            };
            Arc::clone(&guard.registries[idx])
        };
        registry.call_tool(name, arguments).await
    }

    async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            registry.set_call_context(ctx).await;
        }
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            if let Some(schema) = registry.config_schema().await {
                return Some(schema);
            }
        }
        None
    }

    async fn approve_permission(&self, request_id: &str) {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            registry.approve_permission(request_id).await;
        }
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            registry.allow_pattern(action, target_pattern).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::significant_drop_tightening)]
mod tests {
    use super::*;
    use ene_tool_proto::ToolName;
    use std::sync::Mutex;

    struct MockRegistry {
        tools: Vec<ToolSpec>,
        call_log: Arc<Mutex<Vec<(String, String)>>>,
        session_id: Arc<Mutex<Option<String>>>,
    }

    impl MockRegistry {
        fn new(tools: Vec<ToolSpec>) -> Self {
            Self {
                tools,
                call_log: Arc::new(Mutex::new(Vec::new())),
                session_id: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl ToolRegistry for MockRegistry {
        fn list_tools(&self) -> Vec<ToolSpec> {
            self.tools.clone()
        }

        async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
            self.call_log
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.to_string()));
            Ok(format!("{name} executed"))
        }

        async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
            *self.session_id.lock().unwrap() = Some(ctx.conversation_id.clone());
        }
    }

    fn make_tool(name: &str) -> ToolSpec {
        ToolSpec::new(
            ToolName::new(name),
            format!("Tool {name}"),
            serde_json::json!({}),
        )
    }

    #[test]
    fn composite_new_empty() {
        let composite = CompositeToolRegistry::try_new(vec![]).unwrap();
        assert!(composite.list_tools().is_empty());
        assert!(composite.state.read().unwrap().tool_index.is_empty());
    }

    #[test]
    fn composite_aggregates_single_registry() {
        let tools = vec![make_tool("alpha"), make_tool("beta")];
        let registry = MockRegistry::new(tools);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(registry)]).unwrap();
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_str(), "alpha");
        assert_eq!(all[1].name.as_str(), "beta");
    }

    #[test]
    fn composite_aggregates_multiple_registries() {
        let r1 = MockRegistry::new(vec![make_tool("a"), make_tool("b")]);
        let r2 = MockRegistry::new(vec![make_tool("c")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(r1), Arc::new(r2)]).unwrap();
        let all = composite.list_tools();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn composite_duplicate_name_is_hard_error() {
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let result = CompositeToolRegistry::try_new(vec![Arc::new(r1), Arc::new(r2)]);
        assert!(matches!(
            result,
            Err(ToolHostError::DuplicateToolName { .. })
        ));
    }

    #[test]
    fn composite_try_add_registry_duplicate_name_is_hard_error() {
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(r1)]).unwrap();
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let result = composite.try_add_registry(Arc::new(r2));
        assert!(matches!(
            result,
            Err(ToolHostError::DuplicateToolName { .. })
        ));
    }

    #[test]
    fn composite_triple_duplicate_is_hard_error() {
        let r0 = MockRegistry::new(vec![make_tool("dup")]);
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let result = CompositeToolRegistry::try_new(vec![Arc::new(r0), Arc::new(r1), Arc::new(r2)]);
        assert!(matches!(
            result,
            Err(ToolHostError::DuplicateToolName { .. })
        ));
    }

    #[tokio::test]
    async fn composite_call_tool_dispatches() {
        let mock = MockRegistry::new(vec![make_tool("find")]);
        let call_log = Arc::clone(&mock.call_log);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(mock)]).unwrap();
        let result = composite.call_tool("find", r#"{"pattern":"*.rs"}"#).await;
        assert_eq!(result.unwrap(), "find executed");
        let log = call_log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, "find");
        assert_eq!(log[0].1, r#"{"pattern":"*.rs"}"#);
    }

    #[tokio::test]
    async fn composite_call_tool_not_found() {
        let mock = MockRegistry::new(vec![make_tool("exists")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(mock)]).unwrap();
        let result = composite.call_tool("nonexistent", "").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ToolHostError::Protocol(ene_tool_proto::ToolError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn composite_set_call_context_propagates() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let sid1 = Arc::clone(&mock1.session_id);
        let sid2 = Arc::clone(&mock2.session_id);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(mock1), Arc::new(mock2)]).unwrap();
        let ctx = ene_tool_proto::CallContext {
            conversation_id: "conv-1".to_string(),
            turn_id: "turn-1".to_string(),
        };
        composite.set_call_context(&ctx).await;
        assert_eq!(sid1.lock().unwrap().as_deref(), Some("conv-1"));
        assert_eq!(sid2.lock().unwrap().as_deref(), Some("conv-1"));
    }
}
