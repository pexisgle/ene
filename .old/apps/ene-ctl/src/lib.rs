#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI client writes to the terminal"
)]
#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]

pub mod chat;
pub mod core;
pub mod inspect;
pub mod schedule;
pub mod session;
