use clap::Parser;
use ene_daemon::{BootOptions, CoreDaemon};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "ene-core", about = "Ene core daemon")]
struct Args {
    /// Data directory (sessions.db + exclusive lock).
    #[arg(long)]
    data_dir: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let data_dir = args
        .data_dir
        .unwrap_or_else(ene_config::paths::app_data_dir);
    match CoreDaemon::boot(BootOptions::new(data_dir)).await {
        Ok(core) => {
            info!(
                data_dir = %core.data_dir().display(),
                recovered = core.recovery().len(),
                "ene-core ready"
            );
            drop(tokio::signal::ctrl_c().await);
            info!("ene-core shutting down");
        }
        Err(err) => {
            tracing::error!(error = %err, "ene-core failed to start");
            std::process::exit(1);
        }
    }
}
