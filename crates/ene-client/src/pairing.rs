//! Pairing ownership proof: HMAC-SHA256 of the Host nonce keyed by the
//! pairing secret. Lives here so `ene-client` does not depend on
//! `ene-credential` (secret-as-API); `ene-credential` owns verification.

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;

#[must_use]
pub fn pairing_proof_hex(secret: &str, nonce: &str) -> String {
    encode_hex_lower(&compute_pairing_mac(secret, nonce))
}

#[expect(
    clippy::expect_used,
    reason = "HMAC-SHA256 accepts keys of any length; construction failure is an unreachable primitive invariant"
)]
fn compute_pairing_mac(secret: &str, nonce: &str) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(nonce.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn encode_hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}
