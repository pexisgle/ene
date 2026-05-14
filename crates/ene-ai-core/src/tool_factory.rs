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
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            registries: Vec::new(),
        }
    }

    pub fn with_builtin(mut self) -> Self {
        self.registries.push(Box::new(crate::tools::builtin::BuiltinToolRegistry::new()));
        self
    }

    pub fn with_screenshot(mut self, scale_percent: u32) -> Self {
        self.registries.push(Box::new(crate::tools::screenshot::ScreenshotToolRegistry::new(scale_percent)));
        self
    }

    pub fn with_sandbox(mut self, config: crate::sandbox::SandboxConfig) -> Self {
        self.registries.push(Box::new(crate::tools::OpencodeToolRegistry::new(config)));
        self
    }

    pub fn add_registry(mut self, registry: Box<dyn ToolRegistry>) -> Self {
        self.registries.push(registry);
        self
    }

    pub fn build(self) -> Arc<dyn ToolRegistry> {
        Arc::new(crate::tools::composite::CompositeToolRegistry::new(self.registries))
    }
}
