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
        println!("Provider: {}", provider_config.name);
        println!("Model: {}", provider_config.model);
        println!("Base URL: {}", provider_config.base_url);
        println!("Card Path: {}", snapshot.config.character);
        let tool_config = snapshot
            .config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();
        println!("Tool Calling: {}", tool_config.enabled);
        println!("Memory Enabled: {}", mem_config.enabled);
        println!("Embedding Backend: {}", provider_config.embedding.backend);
        if provider_config.embedding.backend.as_str() == "local" {
            let local_emb = &provider_config.embedding.local;
            println!("Local Embedding Model: {}", local_emb.model);
        } else {
            println!(
                "Cloud Embedding Model: {}",
                provider_config.embedding.cloud.model
            );
            println!(
                "Cloud Embedding Dims: {}",
                provider_config.embedding.cloud.dimensions
            );
        }
        if mem_config.enabled {
            println!(
                "Memory DB: {}",
                mem_config
                    .resolve_memory_db_path(&snapshot.config.character)
                    .display()
            );
            println!(
                "Summary Recall Limit: {}",
                session_config.recall_limit
            );
            println!("Similarity Threshold: {}", mem_config.similarity_threshold);
        }
        println!("----------------------");

        Ok(())
    }
}
