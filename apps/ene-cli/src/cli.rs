use clap::Parser;

#[derive(Parser)]
#[command(name = "ene-cli", about = "Interactive CLI for Ene AI")]
pub struct Args {
    #[arg(long, default_missing_value = "")]
    pub tooltest: Option<String>,
}
