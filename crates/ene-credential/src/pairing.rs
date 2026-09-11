//! Device pairing: opaque device identities, pairing records and their
//! repository boundary, and the HMAC-SHA256 ownership proof derived from the
//! one-time pairing secret.

use ene_primitive::{RawId, WallClockWithTz};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::CredentialTechnicalError;

/// Opaque device identity for device pairing.
///
/// Wraps a [`RawId`] with no `From` implementations to or from any other
/// type: a `DeviceId` names one logical device in this crate's pairing
/// records only. It is distinct from the wire `DeviceWireId` carried in
/// `ene-api` envelopes: mapping between wire and domain identities happens in
/// Host composition at the call boundary, never in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub RawId);

/// One paired device: its minted identity, display string, and pairing time.
///
/// `descriptor` is an owner-supplied display string (for example `"phone"`);
/// it carries no secret material, so derived [`core::fmt::Debug`] is safe.
/// `paired_at` records when pairing completed, for display and audit only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceRecord {
    /// Minted at approval; never reused.
    pub id: DeviceId,
    /// Opaque wire projection of this device, minted fresh at approval and
    /// unrelated to [`DeviceId`]'s bytes: the only device string that ever
    /// crosses the wire. Clients echo it; they never derive or resolve it.
    pub wire: String,
    pub descriptor: String,
    pub paired_at: WallClockWithTz,
}

/// One requested-but-not-yet-approved pairing.
///
/// `descriptor` is the owner-supplied display string from the request; it
/// carries no secret material, so derived [`core::fmt::Debug`] is safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingPairing {
    pub descriptor: String,
    pub requested_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DevicePairingStatus {
    /// The descriptor is already paired; carries the existing record.
    Paired {
        /// Existing paired-device record, unchanged.
        device: DeviceRecord,
    },
    /// The descriptor is not yet paired; carries the pending request.
    Pending {
        /// Pending request, newly recorded or previously stored.
        pending: PendingPairing,
    },
}

/// Persistence boundary for device pairing requests and approvals.
///
/// Revocation is explicitly deferred: there is no remove/revoke method, so
/// paired records only accumulate.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait DevicePairingRepository: Send + Sync {
    /// Records a pairing request for `descriptor`.
    ///
    /// Returns [`DevicePairingStatus::Paired`] only when `descriptor` is
    /// already paired (idempotent re-request leaves the stored record
    /// untouched). Otherwise records a pending request — or returns the
    /// existing pending entry when one is already stored — and returns
    /// [`DevicePairingStatus::Pending`].
    ///
    /// Blank-descriptor contract: Host ingress validates that the descriptor
    /// is non-blank before calling. Implementations perform no blank check
    /// themselves: a blank `descriptor` is recorded as pending like any other
    /// string, never rejected here, so callers must not rely on this method
    /// to catch blank input.
    async fn request_pairing(
        &self,
        descriptor: String,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError>;

    /// Approves the pending request for `descriptor`, pairing the device and
    /// issuing its one-time pairing secret.
    ///
    /// On a known pending descriptor this mints a fresh device identity via
    /// [`RawId::new`] and a fresh pairing secret (a second [`RawId::new`]
    /// rendered as UUID text), moves the entry from pending to paired, stores
    /// the record (id, descriptor, and timestamps only — never the secret),
    /// and returns the record together with the secret. An unknown descriptor
    /// yields `Ok(None)` — not an error; the caller maps that outcome to a
    /// clarification request. Re-approving an already-paired descriptor
    /// returns the existing record unchanged (no fresh device identity) with
    /// a freshly minted secret, rotating the previous one.
    ///
    /// Secret custody flow: the trait is secret-free in storage. The approve
    /// caller (Host composition) holds the returned secret in memory,
    /// short-lived, transfers it once over a trusted inlet (approve-time
    /// one-time display / protected client file), and provisions it into its
    /// runtime secret map for later ownership-proof verification. The secret
    /// is never logged, never rendered in `Debug`, and never travels the
    /// wire — only the pairing proof derived from it leaves the device.
    ///
    /// Approval records an Owner decision transported from a trusted inlet;
    /// the repository never decides whether pairing is allowed, it records
    /// the decision it was given.
    async fn approve_pending(
        &self,
        descriptor: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError>;

    /// The only durable wire-to-domain resolution: callers holding an
    /// opaque wire string (proof verification, sender attribution) resolve
    /// it here instead of parsing or deriving it.
    async fn find_device_by_wire(
        &self,
        wire: &str,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError>;
}

/// Pairing ownership proof: HMAC-SHA256 over a single-use nonce, keyed by
/// the pairing secret.
///
/// The secret travels a trusted inlet only: it is shown once at approve time
/// for one-time display, or written to a protected client file. It is never
/// logged, never rendered in `Debug`, and never sent over the wire — only
/// the proof hex leaves the device. The nonce is single-use by caller
/// contract: the Host mints a fresh nonce per challenge and rejects reuse,
/// so a captured proof cannot be replayed.
#[must_use]
pub fn pairing_proof_hex(secret: &str, nonce: &str) -> String {
    encode_hex_lower(&compute_pairing_mac(secret, nonce))
}

/// Verifies a pairing ownership proof against the secret and nonce.
///
/// Hex-decodes `proof` (malformed input yields `false`) and compares the
/// bytes against the recomputed MAC in constant time via `subtle`, so no
/// early exit leaks how much of the proof matched. See
/// [`pairing_proof_hex`] for the secret-inlet and nonce-reuse caller
/// contracts, which apply here unchanged.
#[must_use]
pub fn verify_pairing_proof(secret: &str, nonce: &str, proof: &str) -> bool {
    let Some(decoded) = decode_hex_lower(proof) else {
        return false;
    };
    let recomputed = compute_pairing_mac(secret, nonce);
    bool::from(recomputed.as_slice().ct_eq(decoded.as_slice()))
}

/// Computes the raw HMAC-SHA256 of `nonce` keyed by `secret`.
///
/// HMAC accepts keys of any length, so construction cannot fail; returning a
/// fixed MAC would map a primitive breakage onto a valid-looking proof.
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

/// Decodes lowercase hex; yields [`None`] for odd lengths or any byte
/// outside `0-9`/`a-f` (uppercase included, so only canonical proofs parse).
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
