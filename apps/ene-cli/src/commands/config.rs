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
            .get_section::<ene_runtime::AiConfig>()
            .unwrap_or_default();

        println!("--- Current Config ---");
        let chat = provider_config.resolve_chat().ok();
        println!("Provider: {}", provider_config.tasks.chat.provider);
        println!(
            "Model: {}",
            chat.as_ref()
                .map_or_else(|| "unknown".to_string(), |c| c.model.clone())
        );
        if let Some(chat) = &chat {
            println!("Base URL: {}", chat.base_url);
        }
        println!("Card Path: {}", snapshot.config.character);
        let tool_config = snapshot
            .config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();
        println!("Tool Calling: {}", tool_config.enabled);
        println!("Memory Enabled: {}", mem_config.enabled);
        match provider_config.resolve_embedding() {
            Ok(embed) => {
                let (local_model, _) = embed.local_fields();
                if local_model.is_empty() {
                    let (_, _, cloud_model, dimensions, _) = embed.cloud_fields();
                    println!("Embedding Backend: cloud");
                    println!("Cloud Embedding Model: {cloud_model}");
                    println!("Cloud Embedding Dims: {dimensions}");
                } else {
                    println!("Embedding Backend: local");
                    println!("Local Embedding Model: {local_model}");
                }
            }
            Err(_) => println!("Embedding Backend: unknown"),
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
