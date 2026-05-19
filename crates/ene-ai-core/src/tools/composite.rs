use super::definition::{ToolDefinition, ToolRegistry};
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
                        if let Err(e) = store.upsert_tool_embedding(&tool.name, &current_hash, &embedding) {
                            tracing::warn!("[ToolRAG] Failed to persist embedding for '{}': {}", tool.name, e);
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

    fn list_relevant_tools(&self, query_embedding: Option<&[f32]>, limit: usize) -> Vec<ToolDefinition> {
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

        let all_map: HashMap<String, ToolDefinition> = all_tools
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();

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

    async fn ensure_index_built(
        &self,
        embedder: &dyn crate::embedding::EmbeddingProvider,
        _store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        self.ensure_tool_embeddings(embedder).await
    }
}
