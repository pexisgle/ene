use crate::tools::ToolRegistry;
use std::sync::Arc;

/// ツールレジストリビルダー
///
/// 使用例:
/// ```ignore
/// let registry = ToolRegistryBuilder::new()
///     .with_builtin()
///     .with_screenshot(50)
///     .with_sandbox(sandbox_config)
///     .build();
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

    pub fn with_builtin(mut self) -> Self {
        self.registries
            .push(Box::new(crate::tools::utility::builtin::BuiltinToolRegistry::new()));
        self
    }

    pub fn with_screenshot(mut self, scale_percent: u32) -> Self {
        self.registries.push(Box::new(
            crate::tools::utility::screenshot::ScreenshotToolRegistry::new(scale_percent),
        ));
        self
    }

    pub fn with_sandbox(mut self, config: crate::sandbox::SandboxConfig) -> Self {
        self.registries
            .push(Box::new(crate::tools::core::EneToolRegistry::new(config)));
        self
    }

    pub fn with_sandbox_settings(mut self, settings: &crate::config::AiSandboxSettings) -> Self {
        self.registries
            .push(Box::new(crate::tools::core::EneToolRegistry::new(settings.to_sandbox_config())));
        self
    }

    pub fn add_registry(mut self, registry: Box<dyn ToolRegistry>) -> Self {
        self.registries.push(registry);
        self
    }

    pub fn with_store(mut self, store: Arc<crate::memory::store::MemoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn build(self) -> Arc<dyn ToolRegistry> {
        let composite = crate::tools::composite::CompositeToolRegistry::new(self.registries);
        let composite = match self.store {
            Some(store) => composite.with_store(store),
            None => composite,
        };
        Arc::new(composite)
    }
}
