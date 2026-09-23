use std::path::{Path, PathBuf};

pub(crate) mod device;
pub mod error;
pub(crate) mod frames;
pub(crate) mod incarnation;
mod pairing;
pub(crate) mod session;
mod transport;

pub use error::ClientError;
pub use frames::PreparedRequest;
pub use transport::{Client, ConnectProgress, PendingPairingClient};

pub const DEFAULT_COMPANION_REF: &str = "default";

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
