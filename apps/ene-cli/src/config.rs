use crate::style;
use ene_core::{EneRuntime, MemoryConfig};

pub async fn init() -> EneRuntime {
    let _assets_dir = ene_config::ensure_resource_dirs();

    let mut settings = ene_config::load_settings();
    let provider = settings
        .get_section::<ene_core::ProviderSettings>("provider")
        .unwrap_or_default();

    if !provider.api_key.trim().is_empty() && provider.api_key_source != "keyring" {
        println!(
            "{}",
            style::warning(
                "\n[Security Warning] settings.json に API キーが平文で保存されています。"
            )
        );
        println!(
            "{}",
            style::warning(
                "OS のセキュアな秘密情報ストア（Keyring）に移行することを強く推奨します。"
            )
        );

        let confirm = dialoguer::Confirm::new()
            .with_prompt(
                "API キーを Keyring に安全に移行し、settings.json から平文のキーを削除しますか？",
            )
            .default(true)
            .interact()
            .unwrap_or(false);

        if confirm {
            let service = &provider.api_key_keyring_service;
            let account = &provider.api_key_keyring_account;
            match keyring::Entry::new(service, account) {
                Ok(entry) => {
                    match entry.set_password(&provider.api_key) {
                        Ok(_) => {
                            println!(
                                "{}",
                                style::success(
                                    "[Security] API キーを Keyring に正常に保存しました。"
                                )
                            );
                            let mut new_provider = provider.clone();
                            new_provider.api_key = String::new(); // clear plain text
                            new_provider.api_key_source = "keyring".to_string();
                            let _ = settings.set_section("provider", &new_provider);
                            if let Err(e) = ene_config::save_full_settings(&settings) {
                                eprintln!(
                                    "{}",
                                    style::warning(format!(
                                        "[Security] 設定ファイルの保存に失敗しました: {}",
                                        e
                                    ))
                                );
                            } else {
                                println!(
                                    "{}",
                                    style::success(
                                        "[Security] settings.json を更新し、平文キーを削除しました。"
                                    )
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "{}",
                                style::warning(format!(
                                    "[Security] Keyring への保存に失敗しました: {}",
                                    e
                                ))
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        style::warning(format!("[Security] Keyring の初期化に失敗しました: {}", e))
                    );
                }
            }
        }
    }

    match EneRuntime::init().await {
        Ok(mut runtime) => {
            if let Err(e) = runtime.apply_settings(settings).await {
                eprintln!(
                    "{}",
                    style::warning(format!(
                        "[Runtime] Warning: Failed to initialize AI runtime: {}",
                        e
                    ))
                );
                let empty_settings = ene_config::load_settings();
                runtime
                    .apply_settings(empty_settings)
                    .await
                    .expect("Failed to initialize fallback runtime");
            }
            println!(
                "{}",
                style::header("[Runtime] Unified AI Runtime initialized successfully.")
            );
            let mem_config = runtime
                .settings
                .get_section::<MemoryConfig>("memory")
                .unwrap_or_default();
            if mem_config.enabled {
                println!("{}", style::header("[Memory] Long-term memory enabled."));
                println!(
                    "{}",
                    style::header(format!(
                        "[Memory] DB: {}",
                        mem_config.resolve_memory_db_path().display()
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
            let empty_settings = ene_config::load_settings();
            let mut runtime = EneRuntime::init()
                .await
                .expect("Failed to initialize fallback runtime");
            runtime
                .apply_settings(empty_settings)
                .await
                .expect("Failed to apply fallback settings");
            runtime
        }
    }
}
