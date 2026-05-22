use crate::style;
use ene_ai_core::AiRuntime;

pub async fn init() -> AiRuntime {
    let _assets_dir = ene_ai_core::resources::ensure_resource_dirs();

    let settings = ene_ai_core::config::load_settings();

    match AiRuntime::init(settings).await {
        Ok(runtime) => {
            println!("{}", style::header("[Runtime] Unified AI Runtime initialized successfully."));
            if runtime.settings.memory.enabled {
                println!("{}", style::header("[Memory] Long-term memory enabled."));
                println!(
                    "{}",
                    style::header(format!(
                        "[Memory] DB: {}",
                        runtime.settings.resolve_memory_db_path().display()
                    ))
                );
            }
            runtime
        }
        Err(e) => {
            eprintln!(
                "{}",
                style::warning(format!(
                    "[Runtime] Warning: Failed to initialize AI runtime: {}",
                    e
                ))
            );
            // Fallback: create an empty session and registry
            let empty_settings = ene_ai_core::config::load_settings();
            AiRuntime::init(empty_settings)
                .await
                .expect("Failed to initialize fallback runtime")
        }
    }
}
