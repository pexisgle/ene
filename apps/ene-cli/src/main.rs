mod cli;
mod commands;
mod config;
mod context;
mod repl;
mod stream;
mod style;
use clap::Parser;
use cli::Args;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if let Some(api_key) = args.set_api_key {
        let _ = ene_config::ensure_resource_dirs();
        let mut settings = ene_config::load_settings();
        let provider = settings.get_section::<ene_core::ProviderSettings>("provider").unwrap_or_default();
        let service = &provider.api_key_keyring_service;
        let account = &provider.api_key_keyring_account;

        match keyring::Entry::new(service, account) {
            Ok(entry) => {
                match entry.set_password(&api_key) {
                    Ok(_) => {
                        println!("API キーを Keyring に正常に保存しました。");
                        let mut new_provider = provider.clone();
                        new_provider.api_key = String::new();
                        new_provider.api_key_source = "keyring".to_string();
                        let _ = settings.set_section("provider", &new_provider);
                        if let Err(e) = ene_config::save_full_settings(&settings) {
                            eprintln!("設定ファイルの保存に失敗しました: {}", e);
                            std::process::exit(1);
                        } else {
                            println!("settings.json を更新し、api_key_source を 'keyring' に設定しました。");
                        }
                    }
                    Err(e) => {
                        eprintln!("Keyring への保存に失敗しました: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Keyring の初期化に失敗しました: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let runtime = config::init().await;

    println!("ene Interactive CLI");
    println!("Type '/help' for a list of commands.");

    let mut ctx = context::AppContext { runtime };
    repl::run(&mut ctx).await;
}
