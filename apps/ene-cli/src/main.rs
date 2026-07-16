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

#[tokio::main]
async fn main() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,sea_orm=warn"));
    fmt().with_env_filter(filter).init();
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
