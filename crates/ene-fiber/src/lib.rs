//! Spatiotemporal plugin composition (D-32). Kill is not unload.

#![cfg_attr(
    test,
    expect(clippy::unwrap_used, clippy::expect_used, reason = "tests fail fast")
)]
#![deny(unsafe_code)]

mod broker;
mod fiber;
mod profile;
mod providers;
mod sidecar;
mod spawn;
mod supervisor;

pub use broker::{Broker, BrokerError, Grant, confine_path};
pub use fiber::{Effect, Fiber, FiberState, FiberUid};
pub use profile::ProfileApplyReport;
pub use providers::{
    PROVIDER_PLUGINS, ProviderPlugin, provider_catalog, provider_plugin, task_seam,
};
pub use sidecar::{SidecarHealth, SidecarId, SidecarRequest};
pub use spawn::{
    discover_plugin_bin, discover_plugin_executable, discover_plugin_executable_in,
    discover_plugin_script,
};
pub use supervisor::{
    CircuitBreakerConfig, ProfileRow, Supervisor, SupervisorError, manifest_digest,
};

#[cfg(test)]
mod tests;
