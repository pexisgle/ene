#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI client writes to the terminal"
)]

pub mod core;
pub mod session;
