use crate::style;
use ene_ai_core::{config::AiSettings, init_memory, session::ConversationSession};

pub fn init() -> (AiSettings, ConversationSession) {
    let _assets_dir = ene_ai_core::resources::ensure_resource_dirs();

    let settings = ene_ai_core::config::load_settings();

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
