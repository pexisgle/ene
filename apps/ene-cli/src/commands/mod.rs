mod memory;
mod session;

use crate::context::AppContext;
use ene_core::{MemoryConfig, SessionConfig};

pub async fn execute(input: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "/quit" => std::process::exit(0),
        "/clear" => {
            // Session clearing: just start a new run without saving history
            // The actor handles its own session
            println!("Conversation history will be refreshed on next run.");
        }
        "/prompt" => handle_prompt(ctx).await,
        "/card" => handle_card(arg, ctx),
        "/config" => handle_config(ctx).await,
        "/history" => handle_history(ctx).await,
        "/help" => handle_help(),
        "/undo" => handle_undo(ctx).await,
        "/tool" => handle_tool(arg, ctx).await,
        "/memory" => memory::execute(arg, ctx).await,
        "/session" => session::execute(arg, ctx).await,
        _ => eprintln!("Unknown command: {}", cmd),
    }
}

async fn handle_prompt(ctx: &AppContext) {
    let Ok(snapshot) = ctx.handle.get_snapshot().await else {
        eprintln!("Failed to get actor state");
        return;
    };

    if let Some(card) = &snapshot.character_card {
        let sys = ene_core::message_builder::build_system_prompt(
            card,
            &snapshot.config.runtime_rules,
            &snapshot.config.user_name,
        );
        if !sys.trim().is_empty() {
            println!("--- System Prompt ---");
            println!("{}", sys);
            println!("---------------------");
        }

        if !snapshot.history.is_empty() && !card.data.mes_example.trim().is_empty() {
            println!("--- Example Messages ---");
            let ex = ene_config::expand_cbs_macros(
                &card.data.mes_example,
                card.data.get_character_name(),
                &snapshot.config.user_name,
            );
            println!("{}", ex);
            println!("----------------------------------------------------");
        }

        let mem_config = snapshot
            .config
            .get_section::<MemoryConfig>()
            .unwrap_or_default();
        let session_config = snapshot
            .config
            .get_section::<SessionConfig>()
            .unwrap_or_default();
        if mem_config.enabled {
            println!("--- Recalled Summaries ---");
            println!(
                "Up to {} past conversation summaries will be injected dynamically based on embedding similarity.",
                session_config.summary_recall_limit
            );
            println!("----------------------------");
        }

        if snapshot.memory.is_enabled() {
            let card_name = card.data.get_character_name();
            if let Ok(facts) = snapshot.memory.get_all_keyfacts(card_name) {
                if !facts.is_empty() {
                    println!("--- Known Facts about {} ---", snapshot.config.user_name);
                    for f in facts {
                        println!("• {}: {}", f.key, f.value);
                    }
                    println!("-----------------------------");
                }
            }
        }

        if !snapshot.history.is_empty() {
            println!(
                "--- Conversation History ({} messages) ---",
                snapshot.history.len()
            );
            println!("(Use /history to view full content)");
            println!("----------------------------------------");
        }

        if let Some(phi) = ene_core::message_builder::build_expression_phi(card) {
            let phi_expanded = ene_config::expand_cbs_macros(
                &phi,
                card.data.get_character_name(),
                &snapshot.config.user_name,
            );
            if !phi_expanded.trim().is_empty() {
                println!("--- Expression Protocol (ACT tokens) ---");
                println!("{}", phi_expanded);
                println!("----------------------------------------");
            }
        }
    } else {
        println!("No character card loaded.");
    }
}

fn handle_card(arg: &str, ctx: &AppContext) {
    if arg.is_empty() {
        println!("Usage: /card <path>");
    } else {
        let path = ene_config::resolve_character_path(arg);
        // load_character is async; send it through the channel
        let handle = ctx.handle.clone();
        tokio::spawn(async move {
            match handle.load_character(&path).await {
                Ok(()) => println!("Character card loaded: {}", path),
                Err(e) => eprintln!("Failed to load character card: {}", e),
            }
        });
    }
}

async fn handle_config(ctx: &AppContext) {
    let Ok(snapshot) = ctx.handle.get_snapshot().await else {
        eprintln!("Failed to get actor state");
        return;
    };

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
    match provider_config.embedding_backend.as_str() {
        "local" => {
            let local_emb = ene_core::ProviderConfig::local_embedding(&snapshot.config);
            println!("Local Embedding Model: {}", local_emb.model);
        }
        _ => {
            println!(
                "Cloud Embedding Model: {}",
                provider_config.cloud_embedding_model
            );
            println!(
                "Cloud Embedding Dims: {}",
                provider_config.cloud_embedding_dimensions
            );
        }
    }
    if mem_config.enabled {
        println!(
            "Memory DB: {}",
            mem_config.resolve_memory_db_path().display()
        );
        println!(
            "Summary Recall Limit: {}",
            session_config.summary_recall_limit
        );
        println!("Similarity Threshold: {}", mem_config.similarity_threshold);
    }
    println!("----------------------");
}

async fn handle_history(ctx: &AppContext) {
    let Ok(snapshot) = ctx.handle.get_snapshot().await else {
        eprintln!("Failed to get actor state");
        return;
    };

    println!("--- Conversation History ---");
    for entry in &snapshot.history {
        println!("[{:?}] {}", entry.role, entry.content);
    }
    println!("----------------------------");
}

fn handle_help() {
    println!("Commands:");
    println!("  /card <path>         - Load a new character card");
    println!("  /clear               - Clear conversation history");
    println!("  /history             - View conversation history");
    println!("  /config              - View current AI settings");
    println!("  /tool list           - List available tools");
    println!("  /tool help <name>    - Show tool parameters (JSON Schema)");
    println!("  /tool call <name> <json> - Call a tool directly");
    println!("  /undo                - Undo the most recent agent operation");
    println!("  /memory search <query> - Search memories by query and show hit results");
    println!("  /memory list           - List all stored summaries and keyfacts");
    println!("  /session info        - Show current session info");
    println!("  /session split       - Manually split session");
    println!("  /session summaries   - Show past conversation summaries");
    println!(
        "  /prompt             - View the complete prompt sent to the AI (system, tools, memory, etc.)"
    );
    println!("  /quit                - Exit the CLI");
}

async fn handle_undo(_ctx: &AppContext) {
    eprintln!("[Undo] Not yet supported with actor-based runtime. Use /tool call undo");
}

async fn handle_tool(arg: &str, ctx: &AppContext) {
    let subparts: Vec<&str> = arg.splitn(3, ' ').collect();
    match subparts.first().copied() {
        Some("list") => {
            let Ok(_snapshot) = ctx.handle.get_snapshot().await else {
                eprintln!("Failed to get actor state");
                return;
            };
            println!("Available tools require loading a character card first.");
            println!("Use /tool call <name> <json> to call a tool directly.");
        }
        Some("help") if subparts.len() >= 2 => {
            let _name = subparts[1];
            let Ok(_snapshot) = ctx.handle.get_snapshot().await else {
                eprintln!("Failed to get actor state");
                return;
            };
            println!("Tool help requires the actor to be configured.");
            println!("Use /tool call <name> <json> to call a tool directly.");
        }
        Some("call") if subparts.len() >= 3 => {
            let _name = subparts[1];
            let _arguments = subparts[2];
            eprintln!("[Tool Call] Not yet supported. Use a run command instead.");
        }
        _ => println!("Usage: /tool list | /tool help <name> | /tool call <name> <json>"),
    }
}
