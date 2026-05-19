use crate::style;
use ene_ai_core::{config::AiSettings, init_memory, session::ConversationSession};

pub fn init() -> (AiSettings, ConversationSession) {
    let assets_dir = ene_ai_core::resources::ensure_resource_dirs();
    let config_path = ene_ai_core::paths::config_file_path();

    let mut settings = AiSettings::default();

    if let Ok(json_str) = std::fs::read_to_string(&config_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(ai_section) = v.get("ai") {
                if let Ok(ai) = serde_json::from_value::<AiSettings>(ai_section.clone()) {
                    settings = ai;
                }
            }
            if let Some(card) = v
                .get("character")
                .and_then(|c| c.get("selected_character_card_path"))
                .and_then(|x| x.as_str())
            {
                settings.character_card_path = format!("{}/{}", assets_dir.display(), card);
            }
        }
    }

    if settings.character_card_path.is_empty() {
        settings.character_card_path =
            format!("{}/characters/Alicia/character.json", assets_dir.display());
    }

    let mut session = ConversationSession::new();
    if let Err(e) = session.load_card(&settings.character_card_path) {
        println!("Warning: Failed to load default card: {}", e);
    } else {
        println!("Loaded default card: {}", settings.character_card_path);
    }

    if settings.memory.enabled {
        match init_memory(&settings) {
            Ok((store, embedder)) => {
                session.init_memory(store, embedder);
                println!("{}", style::header("[Memory] Long-term memory enabled."));
                println!(
                    "{}",
                    style::header(format!(
                        "[Memory] DB: {}",
                        settings.resolve_memory_db_path().display()
                    ))
                );
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    style::warning(format!(
                        "[Memory] Warning: Failed to initialize memory: {}",
                        e
                    ))
                );
                eprintln!(
                    "{}",
                    style::warning("[Memory] Continuing without long-term memory.")
                );
            }
        }
    }

    (settings, session)
}
