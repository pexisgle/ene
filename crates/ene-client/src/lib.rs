use std::path::{Path, PathBuf};

pub mod device;
pub mod error;
pub mod frames;
pub mod incarnation;
mod pairing;
pub mod session;
mod transport;

pub use error::ClientError;
pub use frames::PreparedRequest;
pub use pairing::{pairing_proof_hex, verify_pairing_proof};
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
