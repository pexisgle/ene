use crate::commands::{CliCommand, CliError};
use crate::context::AppContext;
use async_trait::async_trait;
use ene_mind::MindConfig;
use ene_store::StoreConfig;

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

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;

        let mem_config = snapshot
            .config
            .get_section::<StoreConfig>()
            .unwrap_or_default();
        let mind = snapshot
            .config
            .get_section::<MindConfig>()
            .unwrap_or_default();
        let provider_config = snapshot
            .config
            .get_section::<ene_runtime::ProviderConfig>()
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
            println!("Typed Recall Limit: {}", mind.memory.recall_result_limit);
            println!(
                "Recall Similarity Threshold: {}",
                mind.memory.recall_similarity_threshold
            );
        }
        println!("----------------------");

        Ok(())
    }
}
