use crate::tools::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;

/// ツールレジストリビルダー
///
/// 使用例:
/// ```ignore
/// let registry = ToolRegistryBuilder::new()
///     .with_builtin()
///     .with_sandbox(sandbox_config)
///     .with_ipc("/tmp/ene-tool-host.sock", &settings.sandbox)
///     .build().await;
/// ```
pub struct ToolRegistryBuilder {
    registries: Vec<Box<dyn ToolRegistry>>,
    store: Option<Arc<crate::memory::store::MemoryStore>>,
    ipc: Option<(PathBuf, ene_tool_proto::SandboxConfigData)>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        Self {
            registries: Vec::new(),
            store: None,
            ipc: None,
        }
    }

    pub fn with_builtin(mut self) -> Self {
        self.registries.push(Box::new(
            crate::tools::utility::builtin::BuiltinToolRegistry::new(),
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
            .push(Box::new(crate::tools::core::EneToolRegistry::new(
                settings.to_sandbox_config(),
            )));
        self
    }

    /// IPC版ツールホストに接続する
    pub fn with_ipc(
        mut self,
        socket_path: impl Into<PathBuf>,
        settings: &crate::config::AiSandboxSettings,
    ) -> Self {
        self.ipc = Some((socket_path.into(), settings.to_sandbox_config_data()));
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

    pub async fn build(self) -> Arc<dyn ToolRegistry> {
        let mut registries = self.registries;

        if let Some((socket_path, sandbox)) = self.ipc {
            match crate::ipc_client::IpcToolRegistry::new(socket_path, sandbox).await {
                Ok(ipc_registry) => {
                    registries.push(Box::new(ipc_registry));
                }
                Err(e) => {
                    eprintln!("[ToolRegistryBuilder] Failed to connect to IPC tool host: {e}");
                }
            }
        }

        let composite = crate::tools::composite::CompositeToolRegistry::new(registries);
        let composite = match self.store {
            Some(store) => composite.with_store(store),
            None => composite,
        };
        Arc::new(composite)
    }
}
