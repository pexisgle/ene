//! Spatiotemporal plugin composition (D-32). Kill is not unload.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

mod broker;
mod fiber;
mod spawn;
mod supervisor;

pub use broker::{Broker, BrokerError, Grant};
pub use fiber::{Effect, Fiber, FiberState, FiberUid};
pub use spawn::discover_plugin_bin;
pub use supervisor::{ProfileRow, Supervisor, SupervisorError};

#[cfg(test)]
mod tests;
