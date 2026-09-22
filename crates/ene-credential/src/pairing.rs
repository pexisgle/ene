use ene_primitive::{RawId, WallClockWithTz};
use hmac::{Hmac, KeyInit as _, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::CredentialTechnicalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub RawId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceRecord {
    pub id: DeviceId,
    pub wire: String,
    pub descriptor: String,
    pub paired_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingPairing {
    pub pending_id: String,
    pub descriptor: String,
    pub requested_at: WallClockWithTz,
    pub origin_connection: String,
}

#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct PairingSecretMaterial(String);

impl PairingSecretMaterial {
    #[must_use]
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(mut self) -> String {
        core::mem::take(&mut self.0)
    }
}

impl core::fmt::Debug for PairingSecretMaterial {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait DevicePairingRepository: Send + Sync {
    async fn request_pairing(
        &self,
        descriptor: String,
        origin_connection: String,
    ) -> Result<PendingPairing, CredentialTechnicalError>;

    async fn approve_pending(
        &self,
        pending_id: &str,
        origin_connection: &str,
    ) -> Result<Option<(DeviceRecord, PairingSecretMaterial)>, CredentialTechnicalError>;

    async fn abandon_pending_by_origin(
        &self,
        origin_connection: &str,
    ) -> Result<(), CredentialTechnicalError>;

    async fn find_device_by_wire(
        &self,
        wire: &str,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    async fn clear_unapproved_pendings(&self) -> Result<(), CredentialTechnicalError>;

    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError>;
}

/// Verifies a pairing ownership proof against the secret and nonce.
///
/// Hex-decodes `proof` (malformed input yields `false`) and compares the
/// bytes against the recomputed MAC in constant time via `subtle`, so no
/// early exit leaks how much of the proof matched. Minting stays the Client's
/// contract — it holds the pairing secret and deliberately does not depend on
/// this crate — while the Host only verifies. The secret is never logged or
/// rendered in `Debug`, initial provisioning uses the authentication-only
/// frame, and the nonce is single-use by caller contract: the Host mints a
/// fresh nonce per challenge and rejects reuse, so a captured proof cannot be
/// replayed.
#[must_use]
pub fn verify_pairing_proof(secret: &str, nonce: &str, proof: &str) -> bool {
    let Some(decoded) = decode_hex_lower(proof) else {
        return false;
    };
    let recomputed = compute_pairing_mac(secret, nonce);
    bool::from(recomputed.as_slice().ct_eq(decoded.as_slice()))
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

pub(crate) fn encode_hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

pub(crate) fn decode_hex_lower(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
