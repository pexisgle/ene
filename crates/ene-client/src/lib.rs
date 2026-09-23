use std::path::Path;

pub(crate) mod device;
pub mod error;
pub(crate) mod frames;
pub(crate) mod host_pin;
pub(crate) mod incarnation;
mod pairing;
pub(crate) mod runtime_info;
pub(crate) mod session;
mod transport;

pub use error::ClientError;
pub use frames::PreparedRequest;
pub use host_pin::trust_host_pin;
pub use transport::{Client, ConnectProgress, PendingPairingClient};

pub const DEFAULT_COMPANION_REF: &str = "default";

/// A serving local Host accepts loopback TCP on the port published in its
/// protected runtime file; a refused connect means it is not serving.
#[must_use]
pub fn host_is_serving(data_dir: &Path) -> bool {
    let Ok(runtime) = runtime_info::load_host_runtime(data_dir) else {
        return false;
    };
    let Some(port) = runtime.local_port() else {
        return false;
    };
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(500)).is_ok()
}

pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
