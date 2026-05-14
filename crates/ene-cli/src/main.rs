mod cli;
mod commands;
mod config;
mod context;
mod registry;
mod repl;
mod stream;
mod style;
mod tooltest;

use clap::Parser;
use cli::Args;

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let (settings, session) = config::init();

    if let Some(prompt) = args.tooltest {
        tooltest::run(&settings, &session, &prompt).await;
    } else {
        println!("Ene Interactive CLI");
        println!("Type '/help' for a list of commands.");

        let registry = registry::build(&settings).await;
        let mut ctx = context::AppContext {
            settings,
            session,
            registry,
            pending_split: None,
        };
        repl::run(&mut ctx).await;
    }
}
