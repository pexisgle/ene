//! Pre-accept handshake: pairing requests, capability negotiation, and
//! ownership-proof verification.
//!
//! These methods run before the connection is authenticated; every answer
//! predates the acceptance that reveals the connection id, so their senders
//! hide it. Every phase transition and the single-use nonce live in the
//! connection table, so a repeat or out-of-phase frame cannot change the
//! negotiated terms or install currentness (IPC §9.3, #1385).

use super::frames::{
    invalid_phase_reject, outgoing_frame, outgoing_frame_pre_auth, stale_reject, unpaired_close,
};
use super::{HostHandle, LiveInput};
use crate::conn::{ChallengeOutcome, ConnectionPhase, InstallOutcome, NonceAdmission};
use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, DisconnectNotice,
    NegotiatedConnection, PairingRequest, PairingResult,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::DeviceWireId;
use ene_companion::CompanionRepository;
use ene_credential::{DevicePairingRepository, DevicePairingStatus};
use ene_plugin_ipc::WireFrame;
use ene_presence::PresenceRepository;
use uuid::Uuid;

impl HostHandle {
    /// Handles one [`PairingRequest`]: deny unauthorized or blank peers, else
    /// record the request durably.
    ///
    /// Denial carries an operational reason only. A non-blank descriptor goes
    /// to
    /// [`request_pairing`](DevicePairingRepository::request_pairing): an
    /// already-paired descriptor re-issues its device key as
    /// [`Paired`](PairingResult::Paired) (the connection table then records
    /// the issued device in the same phase operation), while a fresh
    /// descriptor answers
    /// [`PendingOwnerConfirmation`](PairingResult::PendingOwnerConfirmation)
    /// until the Host-local `approve-device` inlet records the Owner decision.
    /// Blank descriptors are denied with [`Denied`](PairingResult::Denied):
    /// the pairing outcome has no `NeedsClarification` variant, so refusal is
    /// the honest shape. A store failure likewise denies (operational reason
    /// only); the Client retries the same request, which is idempotent.
    pub(super) async fn pair(
        &self,
        frame: &WireFrame,
        request: &PairingRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if !live.peer_uid_ok {
            return vec![denied_pairing(frame, live, "peer user mismatch")];
        }
        let descriptor = request.device_descriptor.trim().to_string();
        if descriptor.is_empty() {
            return vec![denied_pairing(frame, live, "blank device descriptor")];
        }
        match DevicePairingRepository::request_pairing(&self.store, descriptor).await {
            Ok(DevicePairingStatus::Paired { device }) => {
                // The issued key is the stored opaque projection, never the
                // domain identity: the connection layer records this string
                // verbatim and later frames echo it back for resolution
                // through `find_device_by_wire`.
                let Ok(wire) = device.wire.parse() else {
                    return vec![denied_pairing(frame, live, "pairing store unavailable")];
                };
                let device_id = DeviceWireId(wire);
                if !live
                    .authority
                    .note_paired(&live.connection_id, &device.wire)
                {
                    // The phase moved while the store answered (for example
                    // the connection was superseded): never report a pairing
                    // the table did not record.
                    return vec![phase_rejection(frame, live)];
                }
                vec![outgoing_frame_pre_auth(
                    frame,
                    live,
                    WirePayload::PairingResult(PairingResult::Paired { device_id }),
                )]
            }
            Ok(DevicePairingStatus::Pending { .. }) => vec![outgoing_frame_pre_auth(
                frame,
                live,
                WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation),
            )],
            Err(_) => vec![denied_pairing(frame, live, "pairing store unavailable")],
        }
    }

    /// Handles one [`CapabilityAdvertise`]: bind, negotiate, then challenge.
    ///
    /// On a connection that never paired, the frame's `sender.device_id` must
    /// resolve to an existing device in the device store; that resolution,
    /// the device bind, the terms, and the challenge are one phase operation
    /// (`Accepted → Challenged`), so a reconnect never needs a redundant
    /// pairing round trip and an unresolved claim cannot bind anything. On a
    /// freshly paired connection the record's device is already bound
    /// (`Paired → Challenged`). The table writes the terms and the nonce
    /// exactly once: a repeat capability frame answers
    /// [`InvalidHandshakePhase`](ene_api::v1::reject::RejectKind::InvalidHandshakePhase)
    /// and changes neither.
    ///
    /// When no advertised version shares the v1 major, the reply is a single
    /// terminal [`DisconnectNotice`] (the connection closes after it is
    /// written; there is no `IncompatibleProtocol` DTO in `ene-api`).
    /// Capability frames never attach presence: attach happens only on the
    /// submit path, so a negotiating-but-never-submitting peer leaves
    /// attribution untouched.
    pub(super) async fn advertise(
        &self,
        frame: &WireFrame,
        advertise: &CapabilityAdvertise,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let negotiable = advertise
            .supported_protocol
            .iter()
            .any(|candidate| candidate.shares_major_with(&ProtocolVersion::V1));
        if !negotiable {
            let notice = DisconnectNotice {
                reason: String::from("incompatible protocol major"),
            };
            return vec![outgoing_frame_pre_auth(
                frame,
                live,
                WirePayload::DisconnectNotice(notice),
            )];
        }
        let claimed = frame
            .envelope
            .sender
            .device_id
            .as_ref()
            .map(|id| id.0.as_hyphenated().to_string());
        let bind_device = match (&live.paired_device, claimed) {
            (None, Some(claim)) => {
                // Reconnect: the Host resolves the claimed device against the
                // device store (a revoked device would not resolve) before
                // the bind becomes part of the phase operation.
                match DevicePairingRepository::find_device_by_wire(&self.store, &claim).await {
                    Ok(Some(_)) => Some(claim),
                    _ => return vec![unpaired_close(frame, live)],
                }
            }
            // `live_for` already rejected a claim-less frame on a bound
            // connection and a mismatched claim; an unbound, claim-less frame
            // never paired and cannot proceed.
            (None, None) => return vec![unpaired_close(frame, live)],
            (Some(_), _) => None,
        };
        let negotiated = NegotiatedConnection {
            version: ProtocolVersion::V1,
        };
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        match live.authority.note_challenged(
            &live.connection_id,
            bind_device.as_deref(),
            negotiated.clone(),
            nonce.clone(),
        ) {
            ChallengeOutcome::Challenged => vec![
                outgoing_frame_pre_auth(frame, live, WirePayload::NegotiatedConnection(negotiated)),
                outgoing_frame_pre_auth(
                    frame,
                    live,
                    WirePayload::AuthChallenge(AuthChallenge { nonce }),
                ),
            ],
            ChallengeOutcome::Superseded => {
                vec![stale_reject(
                    frame,
                    live,
                    "capability on a superseded connection",
                )]
            }
            ChallengeOutcome::WrongPhase | ChallengeOutcome::Unknown => {
                vec![phase_rejection(frame, live)]
            }
        }
    }

    /// Handles one [`AuthProof`]: verify against the persisted secret and
    /// install currentness, or answer.
    ///
    /// The pending nonce for this connection is consumed single-use in the
    /// challenged phase; a proof in any other phase never reaches here (the
    /// dispatcher answers `InvalidHandshakePhase`) and a second proof after
    /// the nonce was consumed cannot challenge again. A missing device, a
    /// missing or unreadable secret, or a bad proof consumes the nonce and
    /// ends the connection phase in `Closed` with
    /// [`Rejected`](AuthResult::Rejected) — a captured proof can never
    /// replay. Success installs this connection as the device's current
    /// authenticated one in one table section (superseding the previous
    /// current irreversibly) *before* the
    /// [`Accepted`](AuthResult::Accepted) answer is sent, so a lost response
    /// never rolls the install back and a concurrent authentication only wins
    /// by installing later. Proof comparison itself runs in constant time
    /// inside `ene-credential`.
    pub(super) async fn verify_proof(
        &self,
        frame: &WireFrame,
        proof: &AuthProof,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let nonce = match live.authority.take_nonce(&live.connection_id) {
            NonceAdmission::Nonce(nonce) => nonce,
            NonceAdmission::Superseded => {
                return vec![stale_reject(
                    frame,
                    live,
                    "auth proof on a superseded connection",
                )];
            }
            NonceAdmission::Missing | NonceAdmission::WrongPhase | NonceAdmission::Unknown => {
                return vec![invalid_phase_reject(
                    frame,
                    live,
                    "auth proof outside the challenged phase",
                )];
            }
        };
        // Device attribution comes from the connection table (bound moments
        // earlier on this same connection), never from the envelope claim:
        // the proof authenticates the pending pairing the Host recorded, and
        // trusting a Client-supplied device here would let any peer claim
        // any identity. The opaque wire string resolves to its domain record
        // through the store — never by parsing, since projections are
        // unrelated to the domain bytes — and the domain id keys the secret.
        // Device revocation has no store API yet (explicitly deferred in
        // `ene-credential`), so no revoke can interleave between that
        // resolution and the install below; the install still re-checks the
        // phase before installing.
        let reason = match live.paired_device.clone() {
            Some(device) => {
                let stored = DevicePairingRepository::find_device_by_wire(&self.store, &device)
                    .await
                    .unwrap_or_default();
                let verified = stored.is_some_and(|record| {
                    matches!(
                        self.auth_store
                            .verify_device_proof(&record.id, &nonce, &proof.proof),
                        Ok(true)
                    )
                });
                if verified {
                    None
                } else {
                    Some("invalid proof")
                }
            }
            None => Some("unknown device"),
        };
        match reason {
            None => match live.authority.install_authenticated(&live.connection_id) {
                InstallOutcome::Installed => {
                    let mut out = vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::AuthResult(AuthResult::Accepted {
                            connection_id: live.connection_id,
                        }),
                    )];
                    if let Ok(companion) = self.store.ensure_running_companion().await
                        && let Ok(Some(attribution)) =
                            self.store.load_attribution(companion.as_raw()).await
                    {
                        out.push(outgoing_frame(
                            frame,
                            live,
                            WirePayload::PresenceAttribution(attribution_to_wire(
                                self,
                                &attribution,
                            )),
                        ));
                    }
                    out
                }
                InstallOutcome::Superseded => vec![stale_reject(
                    frame,
                    live,
                    "auth proof on a superseded connection",
                )],
                InstallOutcome::WrongPhase | InstallOutcome::Unknown => {
                    vec![invalid_phase_reject(
                        frame,
                        live,
                        "auth proof outside the challenged phase",
                    )]
                }
            },
            Some(reason) => {
                live.authority.note_auth_failed(&live.connection_id);
                vec![outgoing_frame_pre_auth(
                    frame,
                    live,
                    WirePayload::AuthResult(AuthResult::Rejected {
                        reason: reason.to_string(),
                    }),
                )]
            }
        }
    }
}

/// Answers a handshake frame whose phase changed while its handler awaited
/// outside the table section: superseded connections get the typed stale
/// rejection, everything else the typed phase rejection.
fn phase_rejection(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    if live
        .authority
        .phase_of(&live.connection_id)
        .is_some_and(ConnectionPhase::is_superseded)
    {
        stale_reject(frame, live, "handshake on a superseded connection")
    } else {
        invalid_phase_reject(frame, live, "handshake outside its phase")
    }
}

/// Maps a durable attribution to its wire fact: the companion ref renders
/// the handle-issued projection (resolvable back through
/// [`HostHandle::resolve_companion`]), the client ref is a one-way opaque
/// projection, and generation travels as a value copy. Reporting only,
/// never authority: no Host path parses these strings into domain ids —
/// companion refs resolve through the mapping, and Clients must treat both
/// as opaque.
///
/// The client projection reuses the `device_client` recipe — `UUIDv5` over
/// a kind-separated label — so it is stable across restarts without any
/// mapping table, while remaining non-reversible. It stays one-way (rather
/// than mapped) because nothing ever echoes it back: no inbound DTO carries
/// a `ClientWireRef`, so a mapping would be write-only. The Client's
/// operative wire identity remains the device projection plus incarnation,
/// resolved table-side.
fn attribution_to_wire(
    handle: &HostHandle,
    attribution: &ene_presence::PresenceAttribution,
) -> ene_api::v1::presence::PresenceAttributionWire {
    use ene_api::v1::presence::PresenceStateWire;
    use ene_api::v1::refs::{ClientWireRef, CompanionWireRef};
    use ene_presence::PresenceState;
    ene_api::v1::presence::PresenceAttributionWire {
        companion: CompanionWireRef(handle.companion_wire().to_string()),
        state: match attribution.state {
            PresenceState::Present => PresenceStateWire::Present,
            PresenceState::NoActive => PresenceStateWire::NoActive,
            PresenceState::InTransition => PresenceStateWire::InTransition,
            PresenceState::Stopped => PresenceStateWire::Stopped,
            PresenceState::RecoveryWait => PresenceStateWire::RecoveryWait,
        },
        active_client: attribution.active_client.as_ref().map(|client| {
            ClientWireRef(
                Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!(
                        "ene-presence-client:{}",
                        client.as_raw().as_uuid().as_hyphenated()
                    )
                    .as_bytes(),
                )
                .as_hyphenated()
                .to_string(),
            )
        }),
        generation: attribution.generation.as_u64(),
    }
}

fn denied_pairing(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        WirePayload::PairingResult(PairingResult::Denied {
            reason: reason.to_string(),
        }),
    )
}
