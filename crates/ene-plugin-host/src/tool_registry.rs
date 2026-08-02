//! Tool registry trait, composite registry, and deferred call types.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::error::PluginHostError;
use ene_plugin_proto::{
    CallContext, ConfigFieldError, ConfigOption, DeferredStatus, ToolRagProfile, ToolResult,
    ToolSpec,
};

/// Result of a deferred (background) tool call.
///
/// Mirrors [`ene_plugin_proto::DeferredOutcome`] at the host registry layer.
/// A background-capable tool returns [`DeferredCallResult::Deferred`] with a
/// unique `task_id`; any other tool falls back to [`DeferredCallResult::Sync`],
/// carrying the ordinary synchronous result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredCallResult {
    /// The call ran synchronously and produced its final result now.
    Sync(ToolResult),
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
/// Tool RAG indexing and selection is handled by `ene-rag`,
/// not by this trait.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// All currently registered tools.
    fn list_tools(&self) -> Vec<ToolSpec>;

    /// Returns host/RAG metadata profiles for indexed tools.
    ///
    /// Default synthesizes minimal profiles from [`list_tools`](Self::list_tools)
    /// so MCP / legacy registries keep working without an IPC round-trip.
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.list_tools()
            .iter()
            .map(ToolRagProfile::from_tool_spec)
            .collect()
    }

    /// Executes a tool by name with the given JSON arguments from the LLM
    /// and an optional per-call context.
    ///
    /// When `context` is `Some`, it should be applied to this single call
    /// and does not persist for subsequent calls. Registries that do not
    /// support per-call context simply ignore it.
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<ToolResult, PluginHostError>;

    /// Executes a tool in deferred (background) mode.
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
        context: Option<&CallContext>,
    ) -> Result<DeferredCallResult, PluginHostError> {
        Ok(DeferredCallResult::Sync(
            self.call_tool(name, arguments, context).await?,
        ))
    }

    /// Polls the status of a deferred (background) task by id.
    ///
    /// `tool_name` identifies the owning tool so composite registries can
    /// route the poll to the correct sub-registry (task ids are assigned
    /// per tool process and are not globally unique). The default returns
    /// [`DeferredStatus::Unknown`] for registries that do
    /// not support deferral.
    async fn poll_deferred(&self, _tool_name: &str, _task_id: &str) -> DeferredStatus {
        DeferredStatus::Unknown
    }

    /// Cancels a deferred (background) task by id.
    ///
    /// `tool_name` identifies the owning tool for routing in composite
    /// registries. The default is a no-op for registries that do not
    /// support deferral.
    async fn cancel_deferred(&self, _tool_name: &str, _task_id: &str) {}

    /// Sets the call context (conversation + turn identifiers).
    ///
    /// **Deprecated dead path.** Per-call context is now passed directly to
    /// [`call_tool`](Self::call_tool) / [`call_tool_deferred`](Self::call_tool_deferred)
    /// via their `context` argument, which scopes it to a single call instead
    /// of mutating shared connection state. This setter is retained only for
    /// backward compatibility with legacy callers and is a no-op by default;
    /// new code should not invoke it.
    async fn set_call_context(&self, _ctx: &ene_plugin_proto::CallContext) {}

    /// Approves a pending destructive-operation permission request by ID.
    ///
    /// On composite registries this broadcasts the approval to every
    /// sub-registry; a leaf registry approves the request locally. Prefer
    /// [`approve_permission_for`](Self::approve_permission_for) when the
    /// owning tool is known: a permission request originates from a single
    /// tool call, so routing the approval straight to its owner avoids
    /// fanning out to unrelated plugins.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Approves a pending permission request, routed to the sub-registry
    /// that owns `tool_name`.
    ///
    /// A permission request is raised by exactly one tool call, so the
    /// approval only needs to reach the plugin that owns that tool. The
    /// default implementation cannot resolve ownership and therefore falls
    /// back to the [`approve_permission`](Self::approve_permission)
    /// broadcast; composite registries override this to deliver the approval
    /// in a single round-trip.
    async fn approve_permission_for(&self, _tool_name: &str, request_id: &str) {
        self.approve_permission(request_id).await;
    }

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Revokes a previously granted session-wide permission allow pattern.
    async fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Returns the JSON Schema for the configuration this tool accepts.
    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Lists dynamic config options for `path`, or an empty list when unsupported.
    async fn list_config_options(&self, _path: &str) -> Result<Vec<ConfigOption>, PluginHostError> {
        Ok(Vec::new())
    }

    /// Plugin-delegated config validation; empty errors when unsupported
    /// (caller should fall back to host JSON Schema validation).
    async fn validate_config(
        &self,
        _value: &serde_json::Value,
    ) -> Result<Vec<ConfigFieldError>, PluginHostError> {
        Ok(Vec::new())
    }

    /// Migrates a stored config blob, or returns it unchanged when unsupported.
    async fn migrate_config(
        &self,
        from_version: u32,
        value: serde_json::Value,
    ) -> Result<(serde_json::Value, u32), PluginHostError> {
        Ok((value, from_version))
    }

    /// Takes a pending runtime schema-change push, if any.
    fn take_config_schema_changed(&self) -> Option<(Option<serde_json::Value>, u32)> {
        None
    }
}

/// A tool registry that aggregates multiple sub-registries.
///
/// Tool RAG indexing and selection is handled by `ene-rag`.
/// This registry only handles dispatch (list, call, config).
///
/// Name collision across sub-registries is a hard error — per API v1,
/// every tool must have a unique public name.
pub struct CompositeToolRegistry {
    state: parking_lot::RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    /// Tracks external source registries (e.g., MCP servers) by name.
    /// Maps source name to the registry index in `registries`.
    external_sources: HashMap<String, usize>,
    /// Tombstone slots left behind by unregistered external sources.
    dead_indices: HashSet<usize>,
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
            state: parking_lot::RwLock::new(CompositeState {
                registries,
                tool_index,
                external_sources: HashMap::new(),
                dead_indices: HashSet::new(),
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

    /// Read-locks state and calls `f` with the live registries as a slice.
    ///
    /// In the common case (no unregistered external sources) the registries are
    /// handed to `f` directly as a slice with **no per-call allocation**; `f`
    /// clones only the handles it needs. When tombstones exist (an external
    /// source was unregistered), the live registries are collected into a
    /// temporary `Vec` first so dead slots are skipped.
    ///
    /// The read guard is held only for the duration of the (synchronous) `f`
    /// call. Callers that need to `.await` must clone the handles out (e.g. via
    /// `to_vec`) so the guard is dropped before awaiting — holding a synchronous
    /// lock guard across `.await` would deadlock and is not `Send`.
    fn with_registries<R>(&self, f: impl FnOnce(&[Arc<dyn ToolRegistry>]) -> R) -> R {
        let guard = self.state.read();
        if guard.dead_indices.is_empty() {
            f(&guard.registries)
        } else {
            let alive: Vec<Arc<dyn ToolRegistry>> = guard
                .registries
                .iter()
                .enumerate()
                .filter(|(i, _)| !guard.dead_indices.contains(i))
                .map(|(_, r)| Arc::clone(r))
                .collect();
            f(&alive)
        }
    }

    /// Write-locks state and calls `f` with a mutable reference to `CompositeState`.
    fn with_state_mut<R>(&self, f: impl FnOnce(&mut CompositeState) -> R) -> R {
        let mut guard = self.state.write();
        f(&mut guard)
    }

    /// Resolves the owning sub-registry for a tool name.
    fn registry_for(&self, name: &str) -> Result<Arc<dyn ToolRegistry>, PluginHostError> {
        let guard = self.state.read();
        let Some(&idx) = guard.tool_index.get(name) else {
            return Err(PluginHostError::Protocol(
                ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                },
            ));
        };
        if guard.dead_indices.contains(&idx) {
            return Err(PluginHostError::Protocol(
                ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                },
            ));
        }
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

    /// Register tools from an external source (e.g., an MCP server).
    ///
    /// Each tool is added to the composite's index under a new sub-registry
    /// backed by the provided `tool_registry` (which owns the actual tool
    /// implementation). Tools are namespaced internally so they can be
    /// removed together via [`unregister_external`](Self::unregister_external).
    ///
    /// Re-registering an existing `source` atomically replaces its previous
    /// registration: the old tools are validated against, then swapped out,
    /// only after the new tool set is confirmed collision-free. On any
    /// collision the previous registration is left fully intact (no partial
    /// tombstoning).
    ///
    /// # Errors
    /// Returns [`PluginHostError::DuplicateToolName`] when a tool name collides
    /// with an already-registered tool from a *different* source.
    pub fn register_external(
        &self,
        source: String,
        tool_registry: Arc<dyn ToolRegistry>,
    ) -> Result<(), PluginHostError> {
        self.with_state_mut(|state| {
            // Snapshot the tools owned by the previous registration of this
            // source (if any). These names are allowed to be "replaced" and
            // must not be treated as collisions with the new registration.
            let old_idx = state.external_sources.get(&source).copied();
            let old_tool_names: HashSet<String> = old_idx
                .and_then(|idx| state.registries.get(idx))
                .map(|registry| {
                    registry
                        .list_tools()
                        .into_iter()
                        .map(|tool| tool.name.as_str().to_string())
                        .collect()
                })
                .unwrap_or_default();

            // Validate the new tool set against the live index, ignoring the
            // source's own previous tools. Nothing is mutated until this
            // passes, so a collision leaves the old registration intact.
            for tool in tool_registry.list_tools() {
                let name = tool.name.as_str().to_string();
                if state.tool_index.contains_key(&name) && !old_tool_names.contains(&name) {
                    return Err(PluginHostError::DuplicateToolName { tool_name: name });
                }
            }

            if let Some(old_idx) = old_idx {
                state.dead_indices.insert(old_idx);
                for name in &old_tool_names {
                    state.tool_index.remove(name);
                }
            }

            let idx = state.registries.len();
            for tool in tool_registry.list_tools() {
                let name = tool.name.as_str().to_string();
                state.tool_index.insert(name, idx);
            }
            state.registries.push(tool_registry);
            state.external_sources.insert(source, idx);
            Ok(())
        })
    }

    /// Unregister all tools from a source (on disconnect).
    ///
    /// Removes the tools from the index and marks the backing registry
    /// slot as a tombstone so other registries are not shifted.
    pub fn unregister_external(&self, source: &str) {
        self.with_state_mut(|state| {
            if let Some(&idx) = state.external_sources.get(source) {
                if let Some(registry) = state.registries.get(idx) {
                    for tool in registry.list_tools() {
                        state.tool_index.remove(tool.name.as_str());
                    }
                    state.dead_indices.insert(idx);
                }
                state.external_sources.remove(source);
            }
        });
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        let guard = self.state.read();
        let mut tools = Vec::new();
        for (i, registry) in guard.registries.iter().enumerate() {
            if guard.dead_indices.contains(&i) {
                continue;
            }
            tools.extend(registry.list_tools());
        }
        drop(guard);
        tools
    }

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        let guard = self.state.read();
        let mut profiles = Vec::new();
        for (i, registry) in guard.registries.iter().enumerate() {
            if guard.dead_indices.contains(&i) {
                continue;
            }
            profiles.extend(registry.list_rag_profiles());
        }
        drop(guard);
        profiles
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<ToolResult, PluginHostError> {
        let registry = self.registry_for(name)?;
        registry.call_tool(name, arguments, context).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<&CallContext>,
    ) -> Result<DeferredCallResult, PluginHostError> {
        let registry = self.registry_for(name)?;
        registry.call_tool_deferred(name, arguments, context).await
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
        // The sub-registries are independent connections, so fan the calls out
        // concurrently: the worst-case latency is the slowest single plugin,
        // not the sum over every plugin.
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        futures::future::join_all(
            registries
                .iter()
                .map(|registry| registry.set_call_context(ctx)),
        )
        .await;
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

    async fn list_config_options(&self, path: &str) -> Result<Vec<ConfigOption>, PluginHostError> {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            let options = registry.list_config_options(path).await?;
            if !options.is_empty() {
                return Ok(options);
            }
        }
        Ok(Vec::new())
    }

    async fn validate_config(
        &self,
        value: &serde_json::Value,
    ) -> Result<Vec<ConfigFieldError>, PluginHostError> {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            let errors = registry.validate_config(value).await?;
            if !errors.is_empty() {
                return Ok(errors);
            }
        }
        Ok(Vec::new())
    }

    async fn migrate_config(
        &self,
        from_version: u32,
        value: serde_json::Value,
    ) -> Result<(serde_json::Value, u32), PluginHostError> {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        let Some(registry) = registries.first() else {
            return Ok((value, from_version));
        };
        registry.migrate_config(from_version, value).await
    }

    fn take_config_schema_changed(&self) -> Option<(Option<serde_json::Value>, u32)> {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        for registry in &registries {
            if let Some(changed) = registry.take_config_schema_changed() {
                return Some(changed);
            }
        }
        None
    }

    async fn approve_permission(&self, request_id: &str) {
        // Ownership of the request is unknown here, so broadcast — but
        // concurrently, so one slow plugin cannot stall the others.
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        futures::future::join_all(
            registries
                .iter()
                .map(|registry| registry.approve_permission(request_id)),
        )
        .await;
    }

    async fn approve_permission_for(&self, tool_name: &str, request_id: &str) {
        // Route the approval straight to the plugin that owns the tool which
        // raised the request: a single round-trip instead of a broadcast, so
        // an unrelated plugin mid-long-tool-call can no longer delay it.
        //
        // Ownership is resolved against the *current* tool index: if the
        // owning external source is re-registered between the request being
        // raised and the approval arriving, the lookup can return a different
        // live registry that never saw this `request_id`, so the fallback
        // broadcast below is not triggered. That is still equal-or-better than
        // broadcasting to the pre-re-registration set (which would skip the
        // tombstoned original), hence no special handling is needed.
        if let Ok(registry) = self.registry_for(tool_name) {
            // Forward one level deeper so a nested composite sub-registry can
            // route to its own owner; leaf registries fall back to approving
            // locally via the default trait implementation.
            registry.approve_permission_for(tool_name, request_id).await;
        } else {
            // The owning registry is gone (e.g. an external source was
            // unregistered between the request and the approval). Fall
            // back to a broadcast so a still-pending request elsewhere is
            // not silently dropped.
            tracing::warn!(
                component = "CompositeToolRegistry",
                tool = %tool_name,
                request_id = %request_id,
                "Owning registry not found for permission approval; broadcasting"
            );
            self.approve_permission(request_id).await;
        }
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        // Session-wide grants apply to every plugin, so broadcast — but
        // concurrently, so the slowest plugin bounds the latency.
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        futures::future::join_all(
            registries
                .iter()
                .map(|registry| registry.allow_pattern(action, target_pattern)),
        )
        .await;
    }

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        let registries = self.with_registries(<[std::sync::Arc<dyn ToolRegistry>]>::to_vec);
        futures::future::join_all(
            registries
                .iter()
                .map(|registry| registry.revoke_pattern(action, target_pattern)),
        )
        .await;
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
        approvals: Arc<Mutex<Vec<String>>>,
    }

    impl MockRegistry {
        fn new(tools: Vec<ToolSpec>) -> Self {
            Self {
                tools,
                call_log: Arc::new(Mutex::new(Vec::new())),
                session_id: Arc::new(Mutex::new(None)),
                approvals: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ToolRegistry for MockRegistry {
        fn list_tools(&self) -> Vec<ToolSpec> {
            self.tools.clone()
        }

        async fn call_tool(
            &self,
            name: &str,
            arguments: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, PluginHostError> {
            self.call_log
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.to_string()));
            Ok(ene_plugin_proto::ToolResult::text(format!(
                "{name} executed"
            )))
        }

        async fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
            *self.session_id.lock().unwrap() = Some(ctx.conversation_id.clone());
        }

        async fn approve_permission(&self, request_id: &str) {
            self.approvals.lock().unwrap().push(request_id.to_string());
        }
    }

    /// A registry whose control methods stall for a fixed duration, used to
    /// assert that composite broadcasts run concurrently.
    struct SlowRegistry {
        tools: Vec<ToolSpec>,
        delay: std::time::Duration,
    }

    #[async_trait]
    impl ToolRegistry for SlowRegistry {
        fn list_tools(&self) -> Vec<ToolSpec> {
            self.tools.clone()
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, PluginHostError> {
            Ok(ene_plugin_proto::ToolResult::text("ok"))
        }

        async fn approve_permission(&self, _request_id: &str) {
            tokio::time::sleep(self.delay).await;
        }
    }

    /// A registry whose control methods record entry and then block on a
    /// shared barrier, used to prove composite broadcasts run concurrently
    /// without any wall-clock timing.
    struct BarrierRegistry {
        tools: Vec<ToolSpec>,
        barrier: Arc<tokio::sync::Barrier>,
        entered: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ToolRegistry for BarrierRegistry {
        fn list_tools(&self) -> Vec<ToolSpec> {
            self.tools.clone()
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, PluginHostError> {
            Ok(ene_plugin_proto::ToolResult::text("ok"))
        }

        async fn approve_permission(&self, request_id: &str) {
            // Record entry *before* waiting: a barrier of N parties only
            // releases when every party has arrived, so a sequential fan-out
            // would deadlock here (only the first registry would ever arrive)
            // and the test's timeout converts that deadlock into a failure.
            self.entered.lock().unwrap().push(request_id.to_string());
            let _ = self.barrier.wait().await;
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

    #[test]
    fn register_external_replaces_same_source_atomically() {
        let composite = CompositeToolRegistry::try_new(vec![]).unwrap();

        let v1 = MockRegistry::new(vec![make_tool("a.tool1"), make_tool("a.tool2")]);
        composite
            .register_external("mcp.a".to_string(), Arc::new(v1))
            .unwrap();
        let tools = composite.list_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a.tool1"));
        assert!(names.contains(&"a.tool2"));

        let v2 = MockRegistry::new(vec![make_tool("a.tool1"), make_tool("a.tool3")]);
        composite
            .register_external("mcp.a".to_string(), Arc::new(v2))
            .unwrap();
        let tools = composite.list_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"a.tool1"));
        assert!(names.contains(&"a.tool3"));
        assert!(!names.contains(&"a.tool2"));
    }

    #[test]
    fn register_external_collision_keeps_old_registration() {
        let composite = CompositeToolRegistry::try_new(vec![]).unwrap();

        let base = MockRegistry::new(vec![make_tool("shared")]);
        composite
            .register_external("base".to_string(), Arc::new(base))
            .unwrap();

        let other = MockRegistry::new(vec![make_tool("unique")]);
        composite
            .register_external("other".to_string(), Arc::new(other))
            .unwrap();

        // Re-registering "other" with a tool that collides with "base" must
        // fail, and — critically — must leave "other"'s previous tools intact
        // (the old bug tombstoned them before detecting the collision).
        let colliding = MockRegistry::new(vec![make_tool("shared"), make_tool("unique2")]);
        let result = composite.register_external("other".to_string(), Arc::new(colliding));
        assert!(matches!(
            result,
            Err(PluginHostError::DuplicateToolName { .. })
        ));

        let tools = composite.list_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"unique"));
        assert!(!names.contains(&"unique2"));
        assert_eq!(names.iter().filter(|n| **n == "shared").count(), 1);
    }

    #[tokio::test]
    async fn composite_call_tool_dispatches() {
        let mock = MockRegistry::new(vec![make_tool("find")]);
        let call_log = Arc::clone(&mock.call_log);
        let composite = CompositeToolRegistry::try_new(vec![Arc::new(mock)]).unwrap();
        let result = composite
            .call_tool("find", r#"{"pattern":"*.rs"}"#, None)
            .await;
        assert_eq!(result.unwrap().text_for_llm(), "find executed");
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
        let result = composite.call_tool("nonexistent", "", None).await;
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

    #[tokio::test]
    async fn composite_approve_permission_broadcasts_to_all() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let approvals1 = Arc::clone(&mock1.approvals);
        let approvals2 = Arc::clone(&mock2.approvals);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(mock1), Arc::new(mock2)]).unwrap();

        composite.approve_permission("req-broadcast").await;

        assert_eq!(*approvals1.lock().unwrap(), ["req-broadcast"]);
        assert_eq!(*approvals2.lock().unwrap(), ["req-broadcast"]);
    }

    #[tokio::test]
    async fn composite_approve_permission_for_routes_to_owner() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let approvals1 = Arc::clone(&mock1.approvals);
        let approvals2 = Arc::clone(&mock2.approvals);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(mock1), Arc::new(mock2)]).unwrap();

        // The request originated from tool "b", owned by the second registry;
        // only that registry should receive the approval.
        composite.approve_permission_for("b", "req-routed").await;

        assert!(approvals1.lock().unwrap().is_empty());
        assert_eq!(*approvals2.lock().unwrap(), ["req-routed"]);
    }

    #[tokio::test]
    async fn composite_approve_permission_for_unknown_tool_falls_back() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let approvals1 = Arc::clone(&mock1.approvals);
        let approvals2 = Arc::clone(&mock2.approvals);
        let composite =
            CompositeToolRegistry::try_new(vec![Arc::new(mock1), Arc::new(mock2)]).unwrap();

        // No registry owns "ghost"; the approval must fall back to a broadcast
        // rather than being silently dropped.
        composite
            .approve_permission_for("ghost", "req-fallback")
            .await;

        assert_eq!(*approvals1.lock().unwrap(), ["req-fallback"]);
        assert_eq!(*approvals2.lock().unwrap(), ["req-fallback"]);
    }

    #[tokio::test]
    async fn composite_broadcast_latency_is_bounded_by_slowest_registry() {
        // Three registries each stall 200ms. A sequential broadcast would take
        // ~600ms; a concurrent one completes in ~200ms (the slowest single
        // registry).
        let delay = std::time::Duration::from_millis(200);
        let registries: Vec<Arc<dyn ToolRegistry>> = (0..3)
            .map(|i| {
                Arc::new(SlowRegistry {
                    tools: vec![make_tool(&format!("tool{i}"))],
                    delay,
                }) as Arc<dyn ToolRegistry>
            })
            .collect();
        let composite = CompositeToolRegistry::try_new(registries).unwrap();

        let started = tokio::time::Instant::now();
        composite.approve_permission("req-slow").await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "broadcast took {elapsed:?}; expected concurrent fan-out bounded by the \
             slowest registry (~200ms), not the sequential sum (~600ms)"
        );
    }

    #[tokio::test]
    async fn composite_broadcast_runs_concurrently_deterministic() {
        // Three registries each record entry and then block on a 3-party
        // barrier. A barrier releases only once every party has arrived, so
        // the broadcast can only complete if all three registries are entered
        // *concurrently*; a sequential fan-out would leave the first registry
        // waiting forever. The timeout only guards against that deadlock — it
        // is not the assertion, so this test is independent of wall-clock
        // timing.
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let entered: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let registries: Vec<Arc<dyn ToolRegistry>> = (0..3)
            .map(|i| {
                Arc::new(BarrierRegistry {
                    tools: vec![make_tool(&format!("tool{i}"))],
                    barrier: Arc::clone(&barrier),
                    entered: Arc::clone(&entered),
                }) as Arc<dyn ToolRegistry>
            })
            .collect();
        let composite = CompositeToolRegistry::try_new(registries).unwrap();

        let broadcast = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            composite.approve_permission("req-barrier"),
        )
        .await;
        assert!(
            broadcast.is_ok(),
            "broadcast did not run concurrently: a sequential fan-out deadlocks \
             on the barrier and is caught by the timeout"
        );

        let entered = entered.lock().unwrap();
        assert_eq!(entered.len(), 3);
        assert!(entered.iter().all(|id| id == "req-barrier"));
    }
}
