#![expect(
    clippy::arithmetic_side_effects,
    reason = "CLI stream/memory helpers use intentional counter arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "CLI command parsers index into argv and message buffers"
)]

mod commands;
mod config;
mod context;
mod i18n;
mod repl;
mod stream;
mod style;
mod terminal_ui;
mod tree_log;

#[tokio::main]
async fn main() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};

    let ui = terminal_ui::TerminalUi::init_global();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,sea_orm=warn"));
    tracing_subscriber::registry()
        .with(tree_log::TreeLogLayer::new(ui).with_filter(filter))
        .init();

    let handle = match config::init().await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "Fatal: Failed to initialize runtime");
            std::process::exit(1);
        }
    };

    let loader = i18n::loader();
    tracing::info!("{}", i18n_embed_fl::fl!(loader, "welcome"));
    tracing::info!("{}", i18n_embed_fl::fl!(loader, "help-hint"));

    let mut ctx = context::AppContext::new(handle);
    let code = repl::run(&mut ctx).await;
    std::process::exit(code);
}
