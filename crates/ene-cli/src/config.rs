use crate::style;
use ene_ai_core::{config::AiSettings, init_embedding, init_memory_store, session::ConversationSession};

pub fn init() -> (AiSettings, ConversationSession) {
    let _assets_dir = ene_ai_core::resources::ensure_resource_dirs();

    let settings = ene_ai_core::config::load_settings();

    let mut session = ConversationSession::new();
    if let Err(e) = session.load_card(&settings.character_card_path) {
        println!("Warning: Failed to load default card: {}", e);
    } else {
        println!("Loaded default card: {}", settings.character_card_path);
    }

    match init_embedding(&settings) {
        Ok(embedder) => {
            session.embedding_provider = Some(embedder.clone());
            if settings.memory.enabled {
                match init_memory_store(&settings, &*embedder) {
                    Ok(store) => {
                        session.memory_store = Some(store);
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
                    }
                }
            }
        }
        Err(e) => {
            eprintln!(
                "{}",
                style::warning(format!(
                    "[Embedding] Warning: Failed to initialize embedding: {}",
                    e
                ))
            );
        }
    }

    (settings, session)
}
