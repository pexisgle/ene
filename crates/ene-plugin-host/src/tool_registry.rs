//! Tool registry trait, composite registry, and deferred call types.
//!
//! These types were formerly in `ene-tool-host` and are now the canonical
//! definitions for the unified plugin system.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::PluginHostError;
use ene_plugin_proto::{DeferredStatus, ToolRagProfile, ToolSpec};

/// Result of a deferred (background) tool call (#196).
///
/// Mirrors [`ene_plugin_proto::DeferredOutcome`] at the host registry layer.
/// A background-capable tool returns [`DeferredCallResult::Deferred`] with a
/// unique `task_id`; any other tool falls back to [`DeferredCallResult::Sync`],
/// carrying the ordinary synchronous result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredCallResult {
    /// The call ran synchronously and produced its final result now.
    Sync(String),
    /// The call was accepted for background execution under `task_id`.
    Deferred {
        /// Unique identifier for the queued background task.
        task_id: String,
    },
}

/// Unified tool registry interface — abstracts over both built-in IPC tools and MCP tools.
///
/// Implemented by plugin tool registries, [`McpToolRegistry`](crate::McpToolRegistry),
/// and [`CompositeToolRegistry`].
///
/// Tool RAG indexing and selection is handled by `ene-tool-rag`,
/// not by this trait.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// All currently registered tools.
    fn list_tools(&self) -> Vec<ToolSpec>;

    /// Returns host/RAG metadata profiles for indexed tools (#137).
    ///
    /// Default synthesizes minimal profiles from [`list_tools`](Self::list_tools)
    /// so MCP / legacy registries keep working without an IPC round-trip.
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.list_tools()
            .iter()
            .map(ToolRagProfile::from_tool_spec)
            .collect()
    }

    /// Executes a tool by name with the given JSON arguments from the LLM.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, PluginHostError>;

    /// Executes a tool in deferred (background) mode (#196).
    ///
    /// A background-capable tool returns [`DeferredCallResult::Deferred`]
    /// with a `task_id` and delivers the result later out-of-band. The
    /// default implementation runs the call synchronously and wraps the
    /// result in [`DeferredCallResult::Sync`], so registries that do not
    /// support deferral keep working unchanged.
    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredCallResult, PluginHostError> {
        Ok(DeferredCallResult::Sync(
            self.call_tool(name, arguments).await?,
        ))
    }

    /// Polls the status of a deferred (background) task by id (#196).
    ///
    /// `tool_name` identifies the owning tool so composite registries can
    /// route the poll to the correct sub-registry (task ids are assigned
    /// per tool process and are not globally unique). The default returns
    /// [`DeferredStatus::Unknown`] for registries that do
    /// not support deferral.
    async fn poll_deferred(&self, _tool_name: &str, _task_id: &str) -> DeferredStatus {
        DeferredStatus::Unknown
    }

    /// Cancels a deferred (background) task by id (#196).
    ///
    /// `tool_name` identifies the owning tool for routing in composite
    /// registries. The default is a no-op for registries that do not
    /// support deferral.
    async fn cancel_deferred(&self, _tool_name: &str, _task_id: &str) {}

    /// Sets the call context (conversation + turn identifiers).
    async fn set_call_context(&self, _ctx: &ene_plugin_proto::CallContext) {}

    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Revokes a previously granted session-wide permission allow pattern (#177).
    async fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Returns the JSON Schema for the configuration this tool accepts.
    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}

/// A tool registry that aggregates multiple sub-registries.
///
/// Tool RAG indexing and selection is handled by `ene-tool-rag`.
/// This registry only handles dispatch (list, call, config).
///
/// Name collision across sub-registries is a hard error — per API v1 / #135,
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
    /// Returns [`PluginHostError::DuplicateToolName`] when two or more
    /// sub-registries expose a tool with the same name.
    pub fn try_new(registries: Vec<Arc<dyn ToolRegistry>>) -> Result<Self, PluginHostError> {
        let mut tool_index = HashMap::with_capacity(registries.len().saturating_mul(4));
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if tool_index.contains_key(&name) {
                    return Err(PluginHostError::DuplicateToolName { tool_name: name });
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
    #[expect(
        clippy::panic,
        reason = "legacy infallible constructor; prefer try_new for fallible construction"
    )]
    pub fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self {
        match Self::try_new(registries) {
            Ok(composite) => composite,
            Err(PluginHostError::DuplicateToolName { tool_name }) => {
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

    /// Resolves the owning sub-registry for a tool name.
    fn registry_for(&self, name: &str) -> Result<Arc<dyn ToolRegistry>, PluginHostError> {
        let guard = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(&idx) = guard.tool_index.get(name) else {
            return Err(PluginHostError::Protocol(
                ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                },
            ));
        };
        let Some(registry) = guard.registries.get(idx).map(Arc::clone) else {
            return Err(PluginHostError::Protocol(
                ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                },
            ));
        };
        drop(guard);
        Ok(registry)
    }

    /// Adds a sub-registry to the composite.
    ///
    /// # Errors
    /// Returns [`PluginHostError::DuplicateToolName`] when the new registry
    /// contains a tool name that already exists.
    pub fn try_add_registry(&self, registry: Arc<dyn ToolRegistry>) -> Result<(), PluginHostError> {
        self.with_state_mut(|state| {
            let idx = state.registries.len();
            for tool in registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if state.tool_index.contains_key(&name) {
                    return Err(PluginHostError::DuplicateToolName { tool_name: name });
                }
                state.tool_index.insert(name, idx);
            }
            state.registries.push(registry);
            Ok(())
        })
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

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, PluginHostError> {
        let registry = self.registry_for(name)?;
        registry.call_tool(name, arguments).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredCallResult, PluginHostError> {
        let registry = self.registry_for(name)?;
        registry.call_tool_deferred(name, arguments).await
    }

    async fn poll_deferred(&self, tool_name: &str, task_id: &str) -> DeferredStatus {
        match self.registry_for(tool_name) {
            Ok(registry) => registry.poll_deferred(tool_name, task_id).await,
            Err(_) => DeferredStatus::Unknown,
        }
    }

    async fn cancel_deferred(&self, tool_name: &str, task_id: &str) {
        if let Ok(registry) = self.registry_for(tool_name) {
            registry.cancel_deferred(tool_name, task_id).await;
        }
    }

    async fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
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

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            registry.revoke_pattern(action, target_pattern).await;
        }
    }
}

/// Computes a stable hash of the tool definition used for cache invalidation
/// of tool embeddings. Includes name, description, and parameters so that any
/// meaningful change to the LLM-facing `ToolSpec` triggers re-embedding.
pub fn compute_tool_version_hash(tool: &ToolSpec) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tool.name.as_str().as_bytes());
    hasher.update(tool.description.as_bytes());
    hasher.update(tool.parameters.to_string().as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "composite registry unit tests use unwrap and fixed indices"
)]
mod tests {
    use super::*;
    use ene_plugin_proto::ToolName;
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

        async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, PluginHostError> {
            self.call_log
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.to_string()));
            Ok(format!("{name} executed"))
        }

        async fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
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
            Err(PluginHostError::DuplicateToolName { .. })
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
            Err(PluginHostError::DuplicateToolName { .. })
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
            Err(PluginHostError::DuplicateToolName { .. })
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
        drop(log);
    }

    #[tokio::test]
    async fn composite_call_tool_not_found() {
        let mock = MockRegistry::new(vec![make_tool("exists")]);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(mock)]).unwrap();
        let result = composite.call_tool("nonexistent", "").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PluginHostError::Protocol(ene_plugin_proto::ToolError::NotFound { .. })
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
        let ctx = ene_plugin_proto::CallContext {
            conversation_id: "conv-1".to_string(),
            turn_id: "turn-1".to_string(),
        };
        composite.set_call_context(&ctx).await;
        assert_eq!(sid1.lock().unwrap().as_deref(), Some("conv-1"));
        assert_eq!(sid2.lock().unwrap().as_deref(), Some("conv-1"));
    }
}
