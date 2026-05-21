use crate::tools::ToolRegistry;
use std::sync::Arc;

/// ツールレジストリビルダー
///
/// 使用例:
/// ```ignore
/// let registry = ToolRegistryBuilder::new()
///     .build().await;
/// ```
pub struct ToolRegistryBuilder {
    registries: Vec<Box<dyn ToolRegistry>>,
    store: Option<Arc<crate::memory::store::MemoryStore>>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            registries: Vec::new(),
            store: None,
        }
    }

    pub fn add_registry(mut self, registry: Box<dyn ToolRegistry>) -> Self {
        self.registries.push(registry);
        self
    }

    pub fn with_store(mut self, store: Arc<crate::memory::store::MemoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub async fn build(self) -> Arc<dyn ToolRegistry> {
        let composite = crate::tools::CompositeToolRegistry::new(self.registries);
        let composite = match self.store {
            Some(store) => composite.with_store(store),
            None => composite,
        };
        Arc::new(composite)
    }
}