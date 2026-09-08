//! Core process: data-dir lock, session store, interrupt recovery.

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
mod plugin_profile;

pub use boot::{BootOptions, CoreDaemon, CoreError, TaskSecrets};
pub(crate) use boot::{overlay_ai, overlay_plugins};
pub use http::ServerHandle;

#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod openapi_contract;
#[cfg(test)]
mod tests;
