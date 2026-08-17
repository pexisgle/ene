//! Core daemon process: data-dir lock, session store, interrupt recovery.

#![cfg_attr(
    test,
    expect(clippy::unwrap_used, clippy::panic, reason = "tests may fail fast")
)]
#![deny(unsafe_code)]

mod boot;

pub use boot::{BootOptions, CoreDaemon, CoreError};

#[cfg(test)]
mod tests;
