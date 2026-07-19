#![expect(
    clippy::arithmetic_side_effects,
    reason = "CLI stream/memory helpers use intentional counter arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "CLI command parsers index into argv and message buffers"
)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit tests use expect for assertions"
    )
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
    use std::io::{self, IsTerminal};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer, fmt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,sea_orm=warn"));

    if io::stderr().is_terminal() {
        let ui = terminal_ui::TerminalUi::init_global();
        tracing_subscriber::registry()
            .with(tree_log::TreeLogLayer::new(ui).with_filter(filter))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer().with_writer(io::stderr).with_filter(filter))
            .init();
        // Still install TerminalUi so REPL helpers have a sink.
        let _ = terminal_ui::TerminalUi::init_global();
    }

    let handle = match config::init().await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "Fatal: Failed to initialize runtime");
            std::process::exit(1);
        }
    };

    let mut ctx = context::AppContext::new(handle);
    let code = repl::run(&mut ctx).await;
    std::process::exit(code);
}
