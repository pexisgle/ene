use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{ArtifactError, Result};

/// Hex-encoded SHA-256 of `data`.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Verifies that the file at `path` hashes to `expected_hex`.
///
/// Streams the file so large artifacts are not loaded into memory.
pub fn verify_sha256(path: &Path, expected_hex: &str) -> Result<bool> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()) == expected_hex)
}

/// Validates that `digest` is a 64-character lowercase hex string.
pub(crate) fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ArtifactError::InvalidDigest(digest.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn validate_digest_accepts_hex_and_rejects_others() {
        assert!(validate_digest(&sha256_hex(b"x")).is_ok());
        assert!(validate_digest("xyz").is_err());
        assert!(validate_digest(&"a".repeat(63)).is_err());
    }
}
