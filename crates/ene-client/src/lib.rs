use std::path::Path;

pub(crate) mod device;
pub mod error;
pub(crate) mod frames;
pub(crate) mod host_pin;
pub(crate) mod incarnation;
mod pairing;
mod probe;
pub(crate) mod runtime_info;
pub(crate) mod session;
mod transport;
#[cfg(windows)]
mod win_acl;

pub use error::ClientError;
pub use frames::PreparedRequest;
pub use host_pin::trust_host_pin;
#[cfg(any(test, feature = "test-support"))]
pub use transport::TransportProbe;
pub use transport::{Client, ConnectProgress, PendingPairingClient};

pub const DEFAULT_COMPANION_REF: &str = "default";

/// A serving local Host is confirmed by completing the local WSS upgrade
/// against the trusted pin, token, and startup generation; a stale runtime
/// file or an unrelated listener reusing the port is not serving.
#[must_use]
pub fn host_is_serving(data_dir: &Path) -> bool {
    probe::probe_serving_host(data_dir)
}

pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
