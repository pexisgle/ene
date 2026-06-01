use super::registry::ToolRegistry;
use crate::error::ToolError;
use async_trait::async_trait;
use ene_tool_proto::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;

/// A tool registry that aggregates multiple sub-registries and supports RAG-based tool selection.
pub struct CompositeToolRegistry {
    state: std::sync::RwLock<CompositeState>,
    store: std::sync::RwLock<Option<Arc<ene_memory::MemoryStore>>>,
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
                tool_index.entry(tool.name).or_insert(idx);
            }
        }
        Self {
            state: std::sync::RwLock::new(CompositeState {
                registries,
                tool_index,
            }),
            store: std::sync::RwLock::new(None),
        }
    }

    /// Builder method to attach a memory store for tool RAG.
    pub fn with_store(self, store: Arc<ene_memory::MemoryStore>) -> Self {
        {
            let mut guard = self.store.write().unwrap_or_else(|e| e.into_inner());
            *guard = Some(store);
        }
        self
    }

    /// Sets the memory store for tool RAG.
    pub fn set_store(&self, store: Arc<ene_memory::MemoryStore>) {
        let mut guard = self.store.write().unwrap_or_else(|e| e.into_inner());
        *guard = Some(store);
    }

    /// Adds a sub-registry to the composite.
    pub fn add_registry(&self, registry: Arc<dyn ToolRegistry>) {
        let mut state_guard = self.state.write().unwrap_or_else(|e| e.into_inner());
        let idx = state_guard.registries.len();
        for tool in registry.list_tools() {
            state_guard.tool_index.entry(tool.name).or_insert(idx);
        }
        state_guard.registries.push(registry);
    }

    /// Persists tool definition embeddings to SQLite
    /// Compares against existing hashes and re-embeds only those that have changed
    pub async fn ensure_tool_embeddings(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
    ) -> Result<(), ToolError> {
        let store = {
            let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(s) => Arc::clone(s),
                None => return Ok(()),
            }
        };

        let store_clone = Arc::clone(&store);
        let cached: HashMap<String, (String, Vec<f32>)> =
            tokio::task::spawn_blocking(move || match store_clone.list_tool_embeddings() {
                Ok(entries) => entries
                    .into_iter()
                    .map(|(name, hash, emb)| (name, (hash, emb)))
                    .collect(),
                Err(e) => {
                    tracing::warn!("[ToolRAG] Failed to load cached embeddings: {}", e);
                    HashMap::new()
                }
            })
            .await
            .unwrap_or_default();

        let mut indexed = 0usize;
        let mut reused = 0usize;

        // RwLockReadGuard is not Send, so clone registries under lock before async embed operations
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };

        for registry in &registries {
            for tool in registry.list_tools() {
                let current_hash = super::compute_tool_version_hash(&tool);

                if let Some((stored_hash, _emb)) = cached.get(&tool.name) {
                    if stored_hash == &current_hash {
                        reused += 1;
                        continue;
                    }
                }

                let text = tool.embedding_text();
                match embedder.embed(&text).await {
                    Ok(embedding) => {
                        let store_clone = Arc::clone(&store);
                        let tool_name = tool.name.clone();
                        let current_hash = current_hash.clone();
                        let embedding_clone = embedding.clone();

                        tokio::task::spawn_blocking(move || {
                            if let Err(e) = store_clone.upsert_tool_embedding(
                                &tool_name,
                                &current_hash,
                                &embedding_clone,
                            ) {
                                tracing::warn!(
                                    "[ToolRAG] Failed to persist embedding for '{}': {}",
                                    tool_name,
                                    e
                                );
                            }
                        })
                        .await
                        .unwrap_or_default();

                        indexed += 1;
                    }
                    Err(e) => {
                        tracing::warn!("[ToolRAG] Failed to embed tool '{}': {}", tool.name, e);
                    }
                }
            }
        }

        tracing::info!(
            "[ToolRAG] Indexed {} tools ({} new, {} reused from DB)",
            indexed + reused,
            indexed,
            reused
        );
        Ok(())
    }
}

#[async_trait]
impl ToolRegistry for CompositeToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };
        let mut tools = Vec::new();
        for registry in &registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    async fn select_tools(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
        query: &str,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        let all_tools = self.list_tools();
        let store = {
            let guard = self.store.read().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(s) => Arc::clone(s),
                None => return all_tools,
            }
        };

        if let Err(e) = self.ensure_tool_embeddings(embedder).await {
            tracing::warn!("[ToolRAG] Failed to build index: {}", e);
        }

        let query_embedding = match embedder.embed_query(query).await {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!("[ToolRAG] Failed to embed query: {}", e);
                return all_tools;
            }
        };

        let store_clone = Arc::clone(&store);
        let query_embedding_clone = query_embedding.clone();

        let search_results = tokio::task::spawn_blocking(move || {
            match store_clone.search_tools(&query_embedding_clone, limit, 0.0) {
                Ok(results) => results,
                Err(e) => {
                    tracing::warn!("[ToolRAG] Failed to search tools: {}", e);
                    vec![]
                }
            }
        })
        .await
        .unwrap_or_default();

        if search_results.is_empty() {
            return all_tools;
        }

        let all_map: HashMap<String, ToolDefinition> =
            all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();

        search_results
            .into_iter()
            .filter_map(|(name, _similarity)| all_map.get(&name).cloned())
            .collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        let registry = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            match state_guard.tool_index.get(name) {
                Some(&idx) => Arc::clone(&state_guard.registries[idx]),
                None => {
                    return Err(ToolError::ToolExecutionError(format!(
                        "Tool {} not found",
                        name
                    )));
                }
            }
        };
        registry.call_tool(name, arguments).await
    }

    async fn set_session_id(&self, session_id: &str) {
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };
        for registry in &registries {
            registry.set_session_id(session_id).await;
        }
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };
        for registry in &registries {
            if let Some(schema) = registry.config_schema().await {
                return Some(schema);
            }
        }
        None
    }

    async fn approve_permission(&self, request_id: &str) {
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };
        for registry in &registries {
            registry.approve_permission(request_id).await;
        }
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let registries = {
            let state_guard = self.state.read().unwrap_or_else(|e| e.into_inner());
            state_guard.registries.clone()
        };
        for registry in &registries {
            registry.allow_pattern(action, target_pattern).await;
        }
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn ene_provider::EmbeddingProvider,
        _store: Option<&ene_memory::MemoryStore>,
    ) -> Result<(), ToolError> {
        self.ensure_tool_embeddings(embedder).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockRegistry {
        tools: Vec<ToolDefinition>,
        call_log: Arc<Mutex<Vec<(String, String)>>>,
        session_id: Arc<Mutex<Option<String>>>,
    }

    impl MockRegistry {
        fn new(tools: Vec<ToolDefinition>) -> Self {
            Self {
                tools,
                call_log: Arc::new(Mutex::new(Vec::new())),
                session_id: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl ToolRegistry for MockRegistry {
        fn list_tools(&self) -> Vec<ToolDefinition> {
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

    fn make_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("Tool {name}"),
            parameters: serde_json::json!({}),
            category: None,
            keywords: vec![],
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
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
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
        // Both tools appear in the list, but index maps to first
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
        assert!(matches!(
            result.unwrap_err(),
            ToolError::ToolExecutionError(_)
        ));
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
