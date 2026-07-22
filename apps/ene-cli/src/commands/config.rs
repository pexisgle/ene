use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::{context::AppContext, style, util};
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
        "View or update AI settings"
    }

    fn usage(&self) -> &'static str {
        "/config [set <dotted.key> <value>]"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            return show_config(ctx).await;
        }
        let Some((sub, rest)) = trimmed.split_once(' ') else {
            return Err(CliError::UsageError {
                usage: self.usage().to_string(),
            });
        };
        match sub {
            "set" => set_config(rest),
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

async fn show_config(ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
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
        println!("API Key: {}", util::mask_api_key(&chat.api_key));
    } else {
        println!("API Key: {}", util::mask_api_key(""));
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
            if let Some((local_model, _, _)) = embed.local_fields() {
                println!("Embedding Backend: local");
                println!("Local Embedding Model: {local_model}");
            } else if let Some((_, _, cloud_model, dimensions, _)) = embed.cloud_fields() {
                println!("Embedding Backend: cloud");
                println!("Cloud Embedding Model: {cloud_model}");
                println!("Cloud Embedding Dims: {dimensions}");
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
    let issues = ene_ai::validate_settings(&snapshot.config);
    if !issues.is_empty() {
        println!("Validation:");
        for issue in issues {
            println!("  - {}", issue.message());
        }
    }
    println!("----------------------");
    Ok(CommandOutcome::Continue)
}

fn set_config(rest: &str) -> Result<CommandOutcome, CliError> {
    let rest = rest.trim();
    let Some((key, value)) = rest.split_once(' ') else {
        return Err(CliError::UsageError {
            usage: "/config set <dotted.key> <value>".to_string(),
        });
    };
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        return Err(CliError::UsageError {
            usage: "/config set <dotted.key> <value>".to_string(),
        });
    }

    let mut config =
        ene_config::load_config().map_err(|e| CliError::ExecutionFailed(e.to_string()))?;
    config
        .set_path(key, value)
        .map_err(|e| CliError::ExecutionFailed(e.to_string()))?;
    let issues = ene_ai::validate_settings(&config);
    ene_config::save_full_config(&config)
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to save settings: {e}")))?;

    let display_value = if key.to_ascii_lowercase().contains("api_key") {
        util::mask_api_key(value)
    } else {
        value.to_string()
    };
    println!("{} Set `{key}` = {display_value}", style::success("OK"));
    println!("Note: restart the CLI for AI provider changes to take effect.");
    for issue in issues {
        println!("{} {}", style::warning("WARN"), issue.message());
    }
    Ok(CommandOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_api_key_hides_middle() {
        assert_eq!(util::mask_api_key(""), "[not set]");
        assert_eq!(util::mask_api_key("short"), "[redacted]");
        assert_eq!(util::mask_api_key("sk-abcdefghijklmnop"), "sk-…mnop");
    }
}
