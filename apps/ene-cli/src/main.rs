mod cli;
mod commands;
mod config;
mod context;
mod repl;
mod stream;
mod style;
mod tooltest;

use clap::Parser;
use cli::Args;

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let runtime = config::init().await;

    if let Some(prompt) = args.tooltest {
        tooltest::run(&runtime.settings, &runtime.session, &prompt).await;
    } else {
        println!("Ene Interactive CLI");
        println!("Type '/help' for a list of commands.");

        let mut ctx = context::AppContext { runtime };
        repl::run(&mut ctx).await;
    }
}
