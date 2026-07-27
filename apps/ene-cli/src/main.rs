#![expect(
    clippy::arithmetic_side_effects,
    reason = "CLI stream/memory helpers use intentional counter arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "CLI command parsers index into argv and message buffers"
)]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "ene-cli is a terminal REPL; stdout/stderr are its user-facing output, not library logging"
)]
#![cfg_attr(
    test,
    expect(clippy::expect_used, reason = "unit tests use expect for assertions")
)]

mod cli;
mod commands;
mod config;
mod context;
mod i18n;
mod noninteractive;
mod output;
mod repl;
mod runtime_error;
mod stream;
mod style;
mod terminal_ui;
mod tree_log;
mod util;

use clap::Parser;

#[tokio::main]
async fn main() {
    use std::io::{self, IsTerminal};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer, fmt};

    // Parse CLI args first so `--help`/`--version` work without touching
    // config, the runtime, or the terminal UI.
    let args = cli::Cli::parse();

    // Apply the language override before any localized output is produced.
    if let Some(lang) = &args.lang {
        i18n::select_language(lang);
    }

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

    let opts = config::InitOptions {
        config_path: args.config,
        character: args.character,
    };
    let handle = match config::init(&opts).await {
        Ok(h) => h,
        Err(e) => {
            let message = runtime_error::user_message(&e);
            eprintln!(
                "{}",
                style::error(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "init-failed",
                    error = message
                ))
            );
            std::process::exit(1);
        }
    };

    let mut ctx = context::AppContext::new(handle);

    // Dispatch: with a subcommand, run a single non-interactive operation and
    // exit; without one, start the interactive REPL (#186).
    let code = match &args.command {
        Some(cmd) => noninteractive::execute(cmd, &mut ctx).await,
        None => repl::run(&mut ctx).await,
    };
    std::process::exit(code);
}
