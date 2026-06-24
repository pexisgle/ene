mod commands;
mod config;
mod context;
mod repl;
mod stream;
mod style;

#[tokio::main]
async fn main() {
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

    println!("ene Interactive CLI");
    println!("Type '/help' for a list of commands.");

    let mut ctx = context::AppContext::new(handle);
    let code = repl::run(&mut ctx).await;
    std::process::exit(code);
}
