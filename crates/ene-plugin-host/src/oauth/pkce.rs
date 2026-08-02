//! PKCE (RFC 7636) verifier / challenge generation for the authorization flow.

use base64::Engine;
use sha2::{Digest, Sha256};

/// Generates a 256-bit code verifier, base64url-encoded without padding.
///
/// The verifier is used once per flow: it must be sent to the token endpoint
/// with the code, and must never be persisted.
#[must_use]
pub fn verifier() -> String {
    // Two 128-bit draws avoid depending on rand's array-size support; the
    // host already draws u128 for its service tokens.
    let a: u128 = rand::random();
    let b: u128 = rand::random();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(&a.to_le_bytes());
    bytes[16..].copy_from_slice(&b.to_le_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Derives the S256 code challenge for a verifier.
///
/// Plain (`plain`) challenges are deliberately unsupported: they leak the
/// verifier over the authorization redirect.
#[must_use]
pub fn s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests use unwrap/panic for concise failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_base64url_and_unique_per_call() {
        let a = verifier();
        let b = verifier();
        assert_ne!(a, b);
        assert!(!a.contains('+'));
        assert!(!a.contains('/'));
        assert!(!a.contains('='));
    }

    #[test]
    fn challenge_matches_independently_derived_s256() {
        let verifier = verifier();
        let challenge = s256_challenge(&verifier);
        let expected = {
            use base64::Engine as _;
            let digest = Sha256::digest(verifier.as_bytes());
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
        };
        assert_eq!(challenge, expected);
    }
}
