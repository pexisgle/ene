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
/// `pending_id` is the opaque approval key minted at request time; the Owner
/// approves by this id, never by the display descriptor. `descriptor` is the
/// owner-supplied display string from the request; it carries no secret
/// material, so derived [`core::fmt::Debug`] is safe. `origin_connection` is
/// the Host connection that sent the request, binding the approval to it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingPairing {
    pub pending_id: String,
    pub descriptor: String,
    pub requested_at: WallClockWithTz,
    pub origin_connection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DevicePairingStatus {
    /// The polled pending request was already approved; carries the paired
    /// record, unchanged.
    Paired {
        /// Existing paired-device record, unchanged.
        device: DeviceRecord,
    },
    /// The request is not yet approved; carries the pending request (newly
    /// recorded, previously stored, or freshly re-issued after a stale poll).
    Pending {
        /// Pending request the Owner approves by its opaque id.
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
    /// Records a pairing request for `descriptor` from `origin_connection`.
    ///
    /// Every call with `pending_id: None` mints a fresh opaque pending
    /// identity: same-descriptor requests each get their own pending and
    /// their own Owner confirmation, and the descriptor is never an identity
    /// lookup key (#1389). A reconnect of an already-paired device never
    /// reaches here: it resolves its stored `DeviceWireId` through
    /// [`find_device_by_wire`](DevicePairingRepository::find_device_by_wire)
    /// at capability time instead.
    ///
    /// With `pending_id: Some(id)` the caller polls that pending after Owner
    /// approval: an approved id answers [`DevicePairingStatus::Paired`] with
    /// the unchanged record from any connection, a still-waiting id with a
    /// matching descriptor answers [`DevicePairingStatus::Pending`] with the
    /// stored entry only when polled on its origin connection, and an unknown
    /// id (stale after a restart clear, or never issued) mints a fresh pending
    /// so the client converges on the new identity. A waiting id polled from a
    /// new connection likewise mints a fresh pending: the mapping is kept only
    /// until the origin connection ends, so a new connection always opens a
    /// new request while the stored row stays for the Owner decision. A poll
    /// whose descriptor differs from the stored one is rejected: a request id
    /// reused with a different body never resolves to another request's
    /// pending.
    ///
    /// Blank-descriptor contract: Host ingress validates that the descriptor
    /// is non-blank before calling. Implementations perform no blank check
    /// themselves: a blank `descriptor` is recorded as pending like any other
    /// string, never rejected here, so callers must not rely on this method
    /// to catch blank input.
    async fn request_pairing(
        &self,
        descriptor: String,
        origin_connection: String,
        pending_id: Option<String>,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError>;

    /// Approves the pending request `pending_id` issued on `origin_connection`,
    /// pairing the device and issuing its one-time pairing secret.
    ///
    /// The pending delete and the paired insert share one transaction keyed on
    /// both columns (compare-and-swap): only the row with this exact id and
    /// origin pairs, so an unknown id, an already-approved id without a paired
    /// record, or a wrong connection yields `Ok(None)`. An already-approved
    /// id with a surviving paired record returns that record unchanged with a
    /// freshly minted secret (rotation), exactly like the first approval's
    /// secret custody. Re-approval never mints a second device for one
    /// pending: distinct pendings (even with identical descriptors) pair
    /// distinct devices (#1389).
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
        pending_id: &str,
        origin_connection: &str,
    ) -> Result<Option<(DeviceRecord, String)>, CredentialTechnicalError>;

    /// The only durable wire-to-domain resolution: callers holding an
    /// opaque wire string (proof verification, sender attribution) resolve
    /// it here instead of parsing or deriving it.
    async fn find_device_by_wire(
        &self,
        wire: &str,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    /// Drops every still-unapproved pending request.
    ///
    /// The serving Host runs this once at startup, before the listener binds:
    /// a pending that outlived a restart can never authenticate, so a new
    /// connection always opens a new request (#1389). Paired records are
    /// untouched. Read-only opens never call this.
    async fn clear_unapproved_pendings(&self) -> Result<(), CredentialTechnicalError>;

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
