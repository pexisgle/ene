use clap::Parser;

#[derive(Parser)]
#[command(name = "ene-cli", about = "Interactive CLI for Ene AI")]
pub struct Args {
    #[arg(long, default_missing_value = "")]
    pub tooltest: Option<String>,

    #[arg(long, help = "Store the specified API key in OS Keyring and set the source in configuration")]
    pub set_api_key: Option<String>,
}
