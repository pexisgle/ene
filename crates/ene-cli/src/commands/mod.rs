mod memory;
mod session;

use crate::{context::AppContext, style};

pub async fn execute(input: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "/quit" => std::process::exit(0),
        "/clear" => {
            ctx.session.conversation_history.clear();
            println!("Conversation history cleared.");
        }
        "/prompt" => handle_prompt(ctx),
        "/card" => handle_card(arg, ctx),
        "/config" => handle_config(ctx),
        "/history" => handle_history(ctx),
        "/help" => handle_help(),
        "/tools" => handle_tools(ctx),
        "/undo" => handle_undo(ctx).await,
        "/tooltest" => handle_tooltest(arg, ctx).await,
        "/memory" => memory::execute(arg, ctx).await,
        "/session" => session::execute(arg, ctx).await,
        _ => eprintln!("Unknown command: {}", cmd),
    }
}

fn handle_prompt(ctx: &AppContext) {
    if let Some(card) = &ctx.session.character_card {
        let sys = ene_ai_core::prompt_builder::build_system_prompt(
            card,
            &ctx.settings.runtime_rules,
            &ctx.settings.user_name,
        );
        if !sys.trim().is_empty() {
            println!("--- System Prompt ---");
            println!("{}", sys);
            println!("---------------------");
        }

        if !ctx.session.conversation_history.is_empty() && !card.data.mes_example.trim().is_empty()
        {
            println!("--- Example Messages ---");
            let ex = ene_ai_core::character_card::expand_cbs_macros(
                &card.data.mes_example,
                card.data.get_character_name(),
                &ctx.settings.user_name,
            );
            println!("{}", ex);
            println!("----------------------------------------------------");
        }

        if ctx.settings.memory.enabled {
            println!("--- Recalled Summaries ---");
            println!(
                "Up to {} past conversation summaries will be injected dynamically based on embedding similarity.",
                ctx.settings.memory.summary_recall_limit
            );
            println!("----------------------------");
        }

        if let Some(store) = &ctx.session.memory_store {
            let card_name = card.data.get_character_name();
            if let Ok(facts) = store.get_all_keyfacts(card_name) {
                if !facts.is_empty() {
                    println!("--- Known Facts about {} ---", ctx.settings.user_name);
                    for f in facts {
                        println!("• {}: {}", f.key, f.value);
                    }
                    println!("-----------------------------");
                }
            }
        }

        if !ctx.session.conversation_history.is_empty() {
            println!(
                "--- Conversation History ({} messages) ---",
                ctx.session.conversation_history.len()
            );
            println!("(Use /history to view full content)");
            println!("----------------------------------------");
        }

        if let Some(phi) = ene_ai_core::prompt_builder::build_expression_phi(card) {
            let phi_expanded = ene_ai_core::character_card::expand_cbs_macros(
                &phi,
                card.data.get_character_name(),
                &ctx.settings.user_name,
            );
            if !phi_expanded.trim().is_empty() {
                println!("--- Expression Protocol (ACT tokens) ---");
                println!("{}", phi_expanded);
                println!("----------------------------------------");
            }
        }

        if ctx.settings.tool_calling_enabled {
            let tools = ctx.registry.list_tools();
            if !tools.is_empty() {
                println!("--- Tool Definitions ({} tools) ---", tools.len());
                for tool in &tools {
                    println!("- {}", tool.name);
                    println!("  Description: {}", tool.description);
                    println!(
                        "  Parameters: {}",
                        serde_json::to_string_pretty(&tool.parameters).unwrap_or_default()
                    );
                }
                println!("---------------------------------");
            }
        }
    } else {
        println!("No character card loaded.");
    }
}

fn handle_card(arg: &str, ctx: &mut AppContext) {
    if arg.is_empty() {
        println!("Usage: /card <path>");
    } else {
        match ctx.session.load_card(arg) {
            Ok(exprs) => {
                println!(
                    "Card loaded successfully. Found {} expressions.",
                    exprs.len()
                );
                ctx.settings.character_card_path = arg.to_string();
                save_config(ctx);
            }
            Err(e) => eprintln!("Failed to load card: {}", e),
        }
    }
}

fn handle_config(ctx: &AppContext) {
    println!("--- Current Config ---");
    println!("Provider: {}", ctx.settings.provider_name);
    println!("Model: {}", ctx.settings.model);
    println!("Base URL: {}", ctx.settings.base_url);
    println!("Card Path: {}", ctx.settings.character_card_path);
    println!("Tool Calling: {}", ctx.settings.tool_calling_enabled);
    println!("Memory Enabled: {}", ctx.settings.memory.enabled);
    if ctx.settings.memory.enabled {
        println!(
            "Memory DB: {}",
            ctx.settings.resolve_memory_db_path().display()
        );
        println!("Embedding Model: {}", ctx.settings.memory.embedding_model);
        println!(
            "Summary Recall Limit: {}",
            ctx.settings.memory.summary_recall_limit
        );
        println!(
            "Similarity Threshold: {}",
            ctx.settings.memory.similarity_threshold
        );
    }
    println!("----------------------");
}

fn handle_history(ctx: &AppContext) {
    println!("--- Conversation History ---");
    for (role, msg) in &ctx.session.conversation_history {
        println!("[{:?}] {}", role, msg);
    }
    println!("----------------------------");
}

fn handle_help() {
    println!("Commands:");
    println!("  /card <path>     - Load a new character card");
    println!("  /clear           - Clear conversation history");
    println!("  /history         - View conversation history");
    println!("  /config          - View current AI settings");
    println!("  /tools           - List available tools");
    println!("  /tooltest [prompt] - Run a one-shot tool test");
    println!("  /undo            - Undo the most recent agent operation");
    println!("  /memory search <query> - Search memories by query and show hit results");
    println!("  /memory list           - List all stored summaries and keyfacts");
    println!("  /session info    - Show current session info");
    println!("  /session split   - Manually split session");
    println!("  /session summaries - Show past conversation summaries");
    println!(
        "  /prompt          - View the complete prompt sent to the AI (system, tools, memory, etc.)"
    );
    println!("  /quit            - Exit the CLI");
}

fn handle_tools(ctx: &AppContext) {
    println!("{}", style::header("Available Tools:"));
    for tool in ctx.registry.list_tools() {
        println!("- {}: {}", tool.name, tool.description);
    }
}

async fn handle_undo(ctx: &mut AppContext) {
    match ctx.registry.call_tool("undo", "{}").await {
        Ok(result) => println!("{}", style::success(format!("[Undo] {}", result))),
        Err(e) => eprintln!("{}", style::error(format!("[Undo Failed] {}", e))),
    }
}

async fn handle_tooltest(arg: &str, ctx: &mut AppContext) {
    let prompt = if arg.trim().is_empty() {
        "Use the get_current_time tool and reply with the time only.".to_string()
    } else {
        arg.trim().to_string()
    };
    println!(
        "{}",
        style::header(format!("[ToolTest] Running: {}", prompt))
    );
    crate::tooltest::run(&ctx.settings, &ctx.session, &prompt).await;
}

fn save_config(ctx: &AppContext) {
    let config_path = ene_ai_core::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let config = serde_json::json!({
        "ai": ctx.settings,
    });
    if let Err(e) = std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    ) {
        eprintln!("[Config] Failed to save settings: {}", e);
    }
}
