use super::definition::ToolRegistry;
use super::ToolDefinition;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

fn compute_tool_version_hash(tool: &ToolDefinition) -> String {
    use std::hash::{Hash, Hasher};
    let mut state = std::collections::hash_map::DefaultHasher::new();
    tool.name.hash(&mut state);
    tool.description.hash(&mut state);
    tool.keywords.hash(&mut state);
    tool.parameters.to_string().hash(&mut state);
    let hash = state.finish();
    format!("{:x}", hash)
}

pub struct CompositeToolRegistry {
    registries: Vec<Box<dyn ToolRegistry>>,
    tool_index: HashMap<String, usize>,
    store: Option<Arc<crate::memory::store::MemoryStore>>,
}

impl CompositeToolRegistry {
    pub fn new(registries: Vec<Box<dyn ToolRegistry>>) -> Self {
        let mut tool_index = HashMap::with_capacity(registries.len() * 4);
        for (idx, registry) in registries.iter().enumerate() {
            for tool in registry.list_tools() {
                tool_index.entry(tool.name).or_insert(idx);
            }
        }
        Self {
            registries,
            tool_index,
            store: None,
        }
    }

    pub fn with_store(mut self, store: Arc<crate::memory::store::MemoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// ツール定義のembeddingをSQLiteに永続化する。
    /// 既存のハッシュと比較して、変更があるもののみ再embeddingする。
    pub async fn ensure_tool_embeddings(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
    ) -> Result<(), String> {
        let store = match self.store.as_ref() {
            Some(s) => s,
            None => return Ok(()),
        };

        let cached: HashMap<String, (String, Vec<f32>)> = match store.list_tool_embeddings() {
            Ok(entries) => entries
                .into_iter()
                .map(|(name, hash, emb)| (name, (hash, emb)))
                .collect(),
            Err(e) => {
                tracing::warn!("[ToolRAG] Failed to load cached embeddings: {}", e);
                HashMap::new()
            }
        };

        let mut indexed = 0usize;
        let mut reused = 0usize;

        for registry in &self.registries {
            for tool in registry.list_tools() {
                let current_hash = compute_tool_version_hash(&tool);

                if let Some((stored_hash, _emb)) = cached.get(&tool.name) {
                    if stored_hash == &current_hash {
                        reused += 1;
                        continue;
                    }
                }

                let text = tool.embedding_text();
                match embedder.embed(&text).await {
                    Ok(embedding) => {
                        if let Err(e) =
                            store.upsert_tool_embedding(&tool.name, &current_hash, &embedding)
                        {
                            tracing::warn!(
                                "[ToolRAG] Failed to persist embedding for '{}': {}",
                                tool.name,
                                e
                            );
                        }
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
        let mut tools = Vec::with_capacity(self.tool_index.len());
        for registry in &self.registries {
            tools.extend(registry.list_tools());
        }
        tools
    }

    fn list_relevant_tools(
        &self,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Vec<ToolDefinition> {
        let all_tools = self.list_tools();
        let (Some(emb), Some(store)) = (query_embedding, self.store.as_ref()) else {
            return all_tools;
        };

        let search_results = match store.search_tools(emb, limit, 0.0) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("[ToolRAG] Failed to search tools: {}", e);
                return all_tools;
            }
        };

        let all_map: HashMap<String, ToolDefinition> =
            all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();

        search_results
            .into_iter()
            .filter_map(|(name, _similarity)| all_map.get(&name).cloned())
            .collect()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        match self.tool_index.get(name) {
            Some(&idx) => self.registries[idx].call_tool(name, arguments).await,
            None => Err(format!("Tool {} not found", name)),
        }
    }

    async fn set_session_id(&self, session_id: &str) {
        for registry in &self.registries {
            registry.set_session_id(session_id).await;
        }
    }

    async fn ensure_index_built(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
        _store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        self.ensure_tool_embeddings(embedder).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

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

        async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
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
        assert!(composite.tool_index.is_empty());
    }

    #[test]
    fn composite_aggregates_single_registry() {
        let tools = vec![make_tool("alpha"), make_tool("beta")];
        let registry = MockRegistry::new(tools);
        let composite = CompositeToolRegistry::new(vec![Box::new(registry)]);
        let all = composite.list_tools();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "alpha");
        assert_eq!(all[1].name, "beta");
    }

    #[test]
    fn composite_aggregates_multiple_registries() {
        let r1 = MockRegistry::new(vec![make_tool("a"), make_tool("b")]);
        let r2 = MockRegistry::new(vec![make_tool("c")]);
        let composite = CompositeToolRegistry::new(vec![Box::new(r1), Box::new(r2)]);
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
        let composite = CompositeToolRegistry::new(vec![Box::new(r1), Box::new(r2)]);
        let all = composite.list_tools();
        // Both tools appear in the list, but index maps to first
        assert_eq!(all.len(), 2);
        assert_eq!(composite.tool_index.get("dup"), Some(&0));
    }

    #[tokio::test]
    async fn composite_call_tool_dispatches() {
        let mock = MockRegistry::new(vec![make_tool("find")]);
        let call_log = Arc::clone(&mock.call_log);
        let composite = CompositeToolRegistry::new(vec![Box::new(mock)]);
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
        let composite = CompositeToolRegistry::new(vec![Box::new(mock)]);
        let result = composite.call_tool("nonexistent", "").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Tool nonexistent not found");
    }

    #[tokio::test]
    async fn composite_set_session_id_propagates() {
        let mock1 = MockRegistry::new(vec![make_tool("a")]);
        let mock2 = MockRegistry::new(vec![make_tool("b")]);
        let sid1 = Arc::clone(&mock1.session_id);
        let sid2 = Arc::clone(&mock2.session_id);
        let composite = CompositeToolRegistry::new(vec![Box::new(mock1), Box::new(mock2)]);
        composite.set_session_id("sess_main").await;
        assert_eq!(sid1.lock().unwrap().as_deref(), Some("sess_main"));
        assert_eq!(sid2.lock().unwrap().as_deref(), Some("sess_main"));
    }

    #[test]
    fn compute_tool_version_hash_deterministic() {
        let t1 = make_tool("test");
        let h1 = compute_tool_version_hash(&t1);
        let h2 = compute_tool_version_hash(&t1);
        assert_eq!(h1, h2);
    }

    #[test]
    fn compute_tool_version_hash_differs_on_change() {
        let t1 = make_tool("test");
        let mut t2 = make_tool("test");
        t2.description = "different".into();
        let h1 = compute_tool_version_hash(&t1);
        let h2 = compute_tool_version_hash(&t2);
        assert_ne!(h1, h2);
    }
}
