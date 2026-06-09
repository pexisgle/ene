use crate::commands::CliCommand;
use crate::context::AppContext;
use async_trait::async_trait;
use ene_core::{MemoryConfig, SessionConfig};

pub struct ConfigCommand;

#[async_trait]
impl CliCommand for ConfigCommand {
    fn name(&self) -> &'static str {
        "/config"
    }

    fn description(&self) -> &'static str {
        "View current AI settings"
    }

    fn usage(&self) -> &'static str {
        "/config"
    }

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        let mem_config = snapshot
            .config
            .get_section::<MemoryConfig>()
            .unwrap_or_default();
        let session_config = snapshot
            .config
            .get_section::<SessionConfig>()
            .unwrap_or_default();
        let provider_config = snapshot
            .config
            .get_section::<ene_core::ProviderConfig>()
            .unwrap_or_default();

        println!("--- Current Config ---");
        println!("Provider: {}", provider_config.provider_name);
        println!("Model: {}", provider_config.model);
        println!("Base URL: {}", provider_config.base_url);
        println!("Card Path: {}", snapshot.config.character);
        let tool_config = snapshot
            .config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();
        println!("Tool Calling: {}", tool_config.tool_calling_enabled);
        println!("Memory Enabled: {}", mem_config.enabled);
        println!("Embedding Backend: {}", provider_config.embedding_backend);
        if provider_config.embedding_backend.as_str() == "local" {
            let local_emb = snapshot.config.get_section::<ene_core::LocalEmbeddingConfig>().unwrap_or_default();
            println!("Local Embedding Model: {}", local_emb.model);
        } else {
            println!(
                "Cloud Embedding Model: {}",
                provider_config.cloud_embedding_model
            );
            println!(
                "Cloud Embedding Dims: {}",
                provider_config.cloud_embedding_dimensions
            );
        }
        if mem_config.enabled {
            println!(
                "Memory DB: {}",
                mem_config.resolve_memory_db_path(&snapshot.config.character).display()
            );
            println!(
                "Summary Recall Limit: {}",
                session_config.summary_recall_limit
            );
            println!("Similarity Threshold: {}", mem_config.similarity_threshold);
        }
        println!("----------------------");

        Ok(())
    }
}
