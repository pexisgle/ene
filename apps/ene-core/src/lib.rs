//! Core daemon process: data-dir lock, session store, interrupt recovery.

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may fail fast"
    )
)]
#![deny(unsafe_code)]

mod boot;
mod http;

pub use boot::{BootOptions, CoreDaemon, CoreError};
pub use http::ServerHandle;

#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod tests;
