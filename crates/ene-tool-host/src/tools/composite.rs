use super::registry::ToolRegistry;
use crate::ToolHostError;
use async_trait::async_trait;
use ene_tool_proto::ToolSpec;
use std::collections::HashMap;
use std::sync::Arc;

/// A tool registry that aggregates multiple sub-registries.
///
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag).
/// This registry only handles dispatch (list, call, config).
///
/// Name collisions across sub-registries are resolved by prefixing the
/// tool name with `"<source_index>:"` (e.g. `"0:filesystem.read"`).
pub struct CompositeToolRegistry {
    state: std::sync::RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    renamed: std::collections::HashSet<(usize, String)>,
}

impl CompositeToolRegistry {
    /// Creates a new composite tool registry from the given sub-registries.
    ///
    /// Name collisions are resolved by prefixing with `"<idx>:"` so both
    /// tools remain callable.
    ///
    /// # Errors
    /// Returns [`ToolHostError::DuplicateToolName`] only if the prefixed
    /// name itself collides (same-name tool at two different indices).
    pub fn try_new(registries: Vec<Arc<dyn ToolRegistry>>) -> Result<Self, ToolHostError> {
        let mut tool_index = HashMap::with_capacity(registries.len() * 4);
        let mut renamed = std::collections::HashSet::with_capacity(registries.len());
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                let name = tool.name.as_str();
                if tool_index.contains_key(name) {
                    let prefixed = format!("{idx}:{name}");
                    if tool_index.contains_key(&prefixed) {
                        return Err(ToolHostError::DuplicateToolName { name: prefixed });
                    }
                    tool_index.insert(prefixed, idx);
                    renamed.insert((idx, name.to_string()));
                } else {
                    tool_index.insert(name.to_string(), idx);
                }
            }
        }
        Ok(Self {
            state: std::sync::RwLock::new(CompositeState {
                registries,
                tool_index,
                renamed,
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
            Err(ToolHostError::DuplicateToolName { name }) => {
                panic!("Duplicate tool name in CompositeToolRegistry::new: {name}");
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
    /// Name collisions are resolved by prefixing with `"<idx>:"`.
    ///
    /// # Errors
    /// Returns [`ToolHostError::DuplicateToolName`] only if the prefixed
    /// name itself collides.
    pub fn try_add_registry(&self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError> {
        self.with_state_mut(|state| {
            let idx = state.registries.len();
            let mut pending = Vec::new();
            for tool in registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if state.tool_index.contains_key(&name) {
                    let prefixed = format!("{idx}:{name}");
                    if state.tool_index.contains_key(&prefixed) {
                        return Err(ToolHostError::DuplicateToolName { name: prefixed });
                    }
                    pending.push(prefixed);
                    state.renamed.insert((idx, name));
                } else {
                    pending.push(name);
                }
            }
            for name in pending {
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
        if let Err(e) = self.try_add_registry(registry) {
            panic!("CompositeToolRegistry::add_registry failed: {e}");
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
        for (idx, registry) in guard.registries.iter().enumerate() {
            for mut tool in registry.list_tools() {
                if guard
                    .renamed
                    .contains(&(idx, tool.name.as_str().to_string()))
                {
                    let prefixed = format!("{idx}:{}", tool.name.as_str());
                    tool.name = ene_tool_proto::ToolName::new(prefixed);
                }
                tools.push(tool);
            }
        }
        drop(guard);
        tools
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        let (registry, dispatch_name) = {
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
            let prefix = format!("{idx}:");
            let stripped = if name.starts_with(&prefix) {
                &name[prefix.len()..]
            } else {
                name
            };
            (Arc::clone(&guard.registries[idx]), stripped.to_string())
        };
        registry.call_tool(&dispatch_name, arguments).await
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
    fn composite_duplicate_name_gets_prefixed_fallback() {
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(r1), Arc::new(r2)]).unwrap();
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"dup"));
        assert!(names.contains(&"1:dup"));
    }

    #[test]
    fn composite_try_add_registry_resolves_duplicate_with_prefix() {
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(r1)]).unwrap();
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        composite.try_add_registry(Arc::new(r2)).unwrap();
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"dup"));
        assert!(names.contains(&"1:dup"));
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

    #[tokio::test]
    async fn composite_call_tool_with_prefixed_name_dispatches() {
        let mock1 = MockRegistry::new(vec![make_tool("dup")]);
        let mock2 = MockRegistry::new(vec![make_tool("dup")]);
        let log1 = Arc::clone(&mock1.call_log);
        let log2 = Arc::clone(&mock2.call_log);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(mock1), Arc::new(mock2)]).unwrap();

        let result = composite.call_tool("dup", r#"{"v":1}"#).await;
        assert_eq!(result.unwrap(), "dup executed");
        assert_eq!(log1.lock().unwrap().len(), 1);
        assert_eq!(log1.lock().unwrap()[0].0, "dup");

        let result = composite.call_tool("1:dup", r#"{"v":2}"#).await;
        assert_eq!(result.unwrap(), "dup executed");
        assert_eq!(log2.lock().unwrap().len(), 1);
        assert_eq!(log2.lock().unwrap()[0].0, "dup");
    }

    #[test]
    fn composite_triple_duplicate_gets_distinct_prefixes() {
        let r0 = MockRegistry::new(vec![make_tool("dup")]);
        let r1 = MockRegistry::new(vec![make_tool("dup")]);
        let r2 = MockRegistry::new(vec![make_tool("dup")]);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(r0), Arc::new(r1), Arc::new(r2)]).unwrap();
        let all = composite.list_tools();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"dup"));
        assert!(names.contains(&"1:dup"));
        assert!(names.contains(&"2:dup"));
    }
}
