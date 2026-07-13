use crate::commands::CliCommand;
use crate::context::AppContext;
use async_trait::async_trait;
use ene_mind::MindConfig;
use ene_store::StoreConfig;

pub struct PromptCommand;

#[async_trait]
impl CliCommand for PromptCommand {
    fn name(&self) -> &'static str {
        "/prompt"
    }

    fn description(&self) -> &'static str {
        "View the complete prompt sent to the AI (system, tools, memory, etc.)"
    }

    fn usage(&self) -> &'static str {
        "/prompt"
    }

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        if let Some(card) = &snapshot.character_card {
            let prompts = ene_config::PromptLibrary::load("en");
            let sys = ene_runtime::message_builder::build_system_prompt(
                card,
                &snapshot.config.runtime_rules,
                &snapshot.config.user_name,
                &prompts,
            );
            if !sys.trim().is_empty() {
                println!("--- System Prompt ---");
                println!("{sys}");
                println!("---------------------");
            }

            if !snapshot.history.is_empty() && !card.data.mes_example.trim().is_empty() {
                println!("--- Example Messages ---");
                let ex = ene_config::expand_cbs_macros(
                    &card.data.mes_example,
                    card.data.get_character_name(),
                    &snapshot.config.user_name,
                );
                println!("{ex}");
                println!("----------------------------------------------------");
            }

            let mem_config = snapshot
                .config
                .get_section::<StoreConfig>()
                .unwrap_or_default();
            let mind = snapshot
                .config
                .get_section::<MindConfig>()
                .unwrap_or_default();
            if mem_config.enabled {
                println!("--- Typed Memory Recall ---");
                println!(
                    "Up to {} typed memories will be selected by hybrid recall.",
                    mind.memory.recall_result_limit
                );
                println!("--------------------------------");
            }

            if snapshot.memory.is_enabled()
                && let Ok(memories) = snapshot
                    .memory
                    .list_typed_memories(
                        snapshot.card_name.as_str(),
                        None,
                        mind.memory.recall_result_limit,
                    )
                    .await
                && !memories.is_empty()
            {
                println!(
                    "--- Known Typed Memories about {} ---",
                    snapshot.config.user_name
                );
                for memory in memories {
                    println!("• {}: {}", memory.title, memory.content);
                }
                println!("-----------------------------");
            }

            if !snapshot.history.is_empty() {
                println!(
                    "--- Conversation History ({} messages) ---",
                    snapshot.history.len()
                );
                println!("(Use /history to view full content)");
                println!("----------------------------------------");
            }

            if let Some(phi) = ene_runtime::message_builder::build_expression_phi(card, &prompts) {
                let phi_expanded = ene_config::expand_cbs_macros(
                    &phi,
                    card.data.get_character_name(),
                    &snapshot.config.user_name,
                );
                if !phi_expanded.trim().is_empty() {
                    println!("--- Expression Protocol (ACT tokens) ---");
                    println!("{phi_expanded}");
                    println!("----------------------------------------");
                }
            }
        } else {
            println!("No character card loaded.");
        }

        Ok(())
    }
}
