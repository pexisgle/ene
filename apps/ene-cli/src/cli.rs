use clap::Parser;
use std::path::PathBuf;

/// Interactive CLI REPL for chatting with AI characters, testing tools,
/// and managing memory/sessions.
#[derive(Debug, Parser)]
#[command(name = "ene", version, about, long_about = None)]
pub struct Cli {
    /// Path to a settings.json file to load instead of the default location.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Character card name or path to use instead of the configured default.
    #[arg(long, value_name = "NAME")]
    pub character: Option<String>,

    /// UI language override (en or ja). Defaults to system locale.
    #[arg(long, value_name = "LANG")]
    pub lang: Option<String>,
}
