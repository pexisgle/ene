use crate::stream;
use ene_ai_core::{config::AiSettings, run_ai_with_tools, session::ConversationSession};

pub async fn run(settings: &AiSettings, session: &ConversationSession, prompt_override: &str) {
    let prompt = if prompt_override.trim().is_empty() {
        "Use the get_current_time tool and reply with the time only.".to_string()
    } else {
        prompt_override.trim().to_string()
    };

    let mut sandbox_settings = settings.clone();
    sandbox_settings.tool_calling_enabled = true;

    let mut sandbox_session = if session.character_card.is_some() {
        let mut cloned = session.clone();
        cloned.conversation_history.clear();
        cloned.reset_display_buffer();
        cloned
    } else {
        let mut fresh = ConversationSession::new();
        let assets_dir = ene_ai_core::paths::assets_dir();
        let card_path = format!("{}/characters/Alicia/character.json", assets_dir.display());
        if let Err(_err) = fresh.load_card(&sandbox_settings.character_card_path) {
            if let Err(err2) = fresh.load_card(&card_path) {
                println!("Warning: Failed to load default card: {}", err2);
                return;
            }
        }
        fresh
    };

    let registry = match ene_ai_core::build_tool_registry(&sandbox_settings).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[Error] Failed to build tool registry: {}", e);
            return;
        }
    };
    println!("Tool test: tool_calling_enabled = true");
    println!("Tool test: tools = {}", registry.list_tools().len());

    match run_ai_with_tools(&sandbox_settings, &sandbox_session, &prompt, registry).await {
        Ok(stream) => {
            stream::process_stream(stream, &mut sandbox_session).await;
        }
        Err(err) => {
            eprintln!("[Error] Failed to start stream: {}", err);
        }
    }
}
