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
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
    let handle = match config::init().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!(
                "{}",
                style::error(format!("Fatal: Failed to initialize runtime: {e}"))
            );
            std::process::exit(1);
        }
    };

    let loader = i18n::loader();
    println!("{}", i18n_embed_fl::fl!(loader, "welcome"));
    println!("{}", i18n_embed_fl::fl!(loader, "help-hint"));

    let mut ctx = context::AppContext::new(handle);
    let code = repl::run(&mut ctx).await;
    std::process::exit(code);
}
