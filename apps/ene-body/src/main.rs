//! `ene-body` overlay process entry.
//!
//! Stdout is the projection IPC write side when `--ipc-stdio` is used.
//! Diagnostics go to stderr and never echo frame bytes.

use std::io::Write as _;
use std::process::ExitCode;

use ene_body::{parse_endpoint, run};

fn main() -> ExitCode {
    match run_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            if writeln!(stderr, "{error}").is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::from(error.exit_code())
        }
    }
}

fn run_main() -> Result<(), ene_body::BodyError> {
    let endpoint = parse_endpoint(std::env::args_os())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| ene_body::BodyError::Runtime(std::format!("tokio runtime: {error}")))?;
    runtime.block_on(run(endpoint, ene_body::RunOptions { try_gpu: true }))
}
