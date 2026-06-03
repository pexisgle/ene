use super::registry::ToolRegistry;
use crate::error::ToolError;
use async_trait::async_trait;
use ene_tool_proto::ToolSpec;
use std::collections::HashMap;
use std::sync::Arc;

/// A tool registry that aggregates multiple sub-registries.
///
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag).
/// This registry only handles dispatch (list, call, config).
pub struct CompositeToolRegistry {
    state: std::sync::RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
}

impl CompositeToolRegistry {
    /// Creates a new composite tool registry from the given sub-registries.
    pub fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self {
        let mut tool_index = HashMap::with_capacity(registries.len() * 4);
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                tool_index.entry(tool.name.to_string()).or_insert(idx);
            }
        }
        Self {
            state: std::sync::RwLock::new(CompositeState {
                registries,
                tool_index,
            }),
        }
    }

    /// Read-locks state and calls `f` with a reference to the registries slice.
    fn with_registries<R>(&self, f: impl FnOnce(&[Arc<dyn ToolRegistry>]) -> R) -> R {
        let guard = self.state.read().unwrap_or_else(|e| e.into_inner());
        f(&guard.registries)
    }

    /// Write-locks state and calls `f` with a mutable reference to `CompositeState`.
    fn with_state_mut<R>(&self, f: impl FnOnce(&mut CompositeState) -> R) -> R {
        let mut guard = self.state.write().unwrap_or_else(|e| e.into_inner());
        f(&mut *guard)
    }

    /// Adds a sub-registry to the composite.
    pub fn add_registry(&self, registry: Arc<dyn ToolRegistry>) {
        self.with_state_mut(|state| {
            let idx = state.registries.len();
            for tool in registry.list_tools() {
                state.tool_index.entry(tool.name.to_string()).or_insert(idx);
            }
            state.registries.push(registry);
        });
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        let mut tools = Vec::new();
        self.with_registries(|registries| {
            for registry in registries {
                tools.extend(registry.list_tools());
            }
        });
        tools
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        let registry = {
            let guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            match guard.tool_index.get(name) {
                Some(&idx) => Arc::clone(&guard.registries[idx]),
                None => {
                    return Err(ToolError::NotFound {
                        tool_name: name.to_string(),
                    });
                }
            }
        };
        registry.call_tool(name, arguments).await
    }

    async fn set_session_id(&self, session_id: &str) {
        let registries = self.with_registries(|r| r.to_vec());
        for registry in &registries {
            registry.set_session_id(session_id).await;
        }
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        let registries = self.with_registries(|r| r.to_vec());
        for registry in &registries {
            if let Some(schema) = registry.config_schema().await {
                return Some(schema);
            }
        }
        None
    }

    async fn approve_permission(&self, request_id: &str) {
        let registries = self.with_registries(|r| r.to_vec());
        for registry in &registries {
            registry.approve_permission(request_id).await;
        }
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let registries = self.with_registries(|r| r.to_vec());
        for registry in &registries {
            registry.allow_pattern(action, target_pattern).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_tool_proto::{KeywordSet, SideEffects, ToolCategory, ToolName, ToolVersion};
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

        async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
            self.call_log
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.to_string()));
            Ok(format!("{name} executed"))
        }

        async fn set_session_id(&self, session_id: &str) {
            *self.session_id.lock().unwrap() = Some(session_id.to_string());
        }
    }

    fn make_tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: ToolName::new(name),
            version: ToolVersion::default(),
            display_name: format!("Tool {name}"),
            summary: format!("Tool {name}"),
            description: format!("Tool {name}"),
            category: ToolCategory::Utility,
            keywords: KeywordSet::default(),
            parameters: serde_json::json!({}),
            examples: Vec::new(),
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    #[test]
    fn composite_new_empty() {
        let composite = CompositeToolRegistry::new(vec![]);
        assert!(composite.list_tools().is_empty());
        assert!(composite.state.read().unwrap().tool_index.is_empty());
    }

    #[test]
    fn composite_aggregates_single_registry() {
        let tools = vec![make_tool("alpha"), make_tool("beta")];
        let registry = MockRegistry::new(tools);
        let composite = CompositeToolRegistry::new(vec![Arc::new(registry)]);
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name.as_str(), "alpha");
        assert_eq!(all[1].name.as_str(), "beta");
    }

    #[test]
    fn composite_aggregates_multiple_registries() {
        let r1 = MockRegistry::new(vec![make_tool("a"), make_tool("b")]);
        let r2 = MockRegistry::new(vec![make_tool("c")]);
        let composite = CompositeToolRegistry::new(vec![Arc::new(r1), Arc::new(r2)]);
        let all = composite.list_tools();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn composite_duplicate_name_first_wins() {
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let composite = CompositeToolRegistry::new(vec![Arc::new(r1), Arc::new(r2)]);
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        assert_eq!(
            composite.state.read().unwrap().tool_index.get("dup"),
            Some(&0)
        );
    }

    #[tokio::test]
    async fn composite_call_tool_dispatches() {
        let mock = MockRegistry::new(vec![make_tool("find")]);
        let call_log = Arc::clone(&mock.call_log);
        let composite = CompositeToolRegistry::new(vec![Arc::new(mock)]);
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
        let composite = CompositeToolRegistry::new(vec![Arc::new(mock)]);
        let result = composite.call_tool("nonexistent", "").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn composite_set_session_id_propagates() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let sid1 = Arc::clone(&mock1.session_id);
        let sid2 = Arc::clone(&mock2.session_id);
        let composite = CompositeToolRegistry::new(vec![Arc::new(mock1), Arc::new(mock2)]);
        composite.set_session_id("sess_main").await;
        assert_eq!(sid1.lock().unwrap().as_deref(), Some("sess_main"));
        assert_eq!(sid2.lock().unwrap().as_deref(), Some("sess_main"));
    }
}
