//! Spatiotemporal plugin composition (D-32). Kill is not unload.

#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests fail fast"
    )
)]
#![deny(unsafe_code)]

mod broker;
mod fiber;
mod profile;
mod spawn;
mod supervisor;

pub use broker::{Broker, BrokerError, Grant};
pub use fiber::{Effect, Fiber, FiberState, FiberUid};
pub use profile::ProfileApplyReport;
pub use spawn::{discover_plugin_bin, discover_plugin_executable, discover_plugin_script};
pub use supervisor::{
    CircuitBreakerConfig, ProfileRow, Supervisor, SupervisorError, manifest_digest,
};

#[cfg(test)]
mod tests;
