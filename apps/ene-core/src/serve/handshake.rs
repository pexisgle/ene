//! Pre-accept handshake: pairing requests, capability negotiation, and
//! ownership-proof verification.
//!
//! These methods run before the connection is authenticated; every answer
//! predates the acceptance that reveals the connection id, so their senders
//! hide it.

use super::frames::{outgoing_frame, outgoing_frame_pre_auth};
use super::{HostHandle, LiveInput, conn_key, lock_map};
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
    /// [`Paired`](PairingResult::Paired) (the connection layer then marks the
    /// connection paired), while a fresh descriptor is recorded pending and
    /// answers
    /// [`PendingOwnerConfirmation`](PairingResult::PendingOwnerConfirmation)
    /// until the Host-local `approve-device` inlet records the Owner
    /// decision. Ingress trims surrounding whitespace and denies blank
    /// descriptors with [`Denied`](PairingResult::Denied): the pairing
    /// outcome has no `NeedsClarification` variant, so refusal is the honest
    /// shape. A store failure likewise denies (operational reason only); the
    /// Client retries the same request, which is idempotent. Every answer
    /// here predates authentication, so its sender hides the connection id.
    pub(super) async fn pair(
        &self,
        frame: &WireFrame,
        request: &PairingRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if !live.peer_uid_ok {
            return vec![denied_pairing(frame, live, "peer user mismatch")];
        }
        // One connection pairs with one device for its lifetime: a second
        // request must not be able to move the connection (and its liveness
        // and auth binding) to another device. The connection table also
        // enforces set-once, so both layers refuse the move.
        if live.paired_device.is_some() {
            return vec![denied_pairing(frame, live, "connection already paired")];
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

    /// Handles one [`CapabilityAdvertise`]: negotiate, then challenge.
    ///
    /// When no advertised version shares the v1 major, the reply is a single
    /// terminal [`DisconnectNotice`] (the connection closes after it is
    /// written; there is no `IncompatibleProtocol` DTO in `ene-api`).
    /// Otherwise the reply carries the negotiated terms (version v1 and every
    /// advertised feature kind as receipt, never as permission) plus a fresh
    /// [`AuthChallenge`] whose nonce is recorded pending for this connection:
    /// the Client answers with an [`AuthProof`] proving possession of its
    /// pairing secret. Re-advertising replaces the pending nonce, so only the
    /// latest challenge can be answered. Both answers predate authentication,
    /// so their senders hide the connection id. Capability frames never
    /// attach presence: attach happens only on the submit path, so a
    /// negotiating-but-never-submitting peer leaves attribution untouched.
    pub(super) fn advertise(
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
        let accepted = advertise
            .features
            .iter()
            .map(|feature| feature.kind)
            .collect();
        let negotiated = NegotiatedConnection {
            version: ProtocolVersion::V1,
            accepted_features: accepted,
        };
        let nonce = Uuid::new_v4().as_hyphenated().to_string();
        lock_map(&self.pending_nonces).insert(conn_key(&live.connection_id), nonce.clone());
        vec![
            outgoing_frame_pre_auth(frame, live, WirePayload::NegotiatedConnection(negotiated)),
            outgoing_frame_pre_auth(
                frame,
                live,
                WirePayload::AuthChallenge(AuthChallenge { nonce }),
            ),
        ]
    }

    /// Handles one [`AuthProof`]: verify against the persisted secret and answer.
    ///
    /// No prior auth is required: this frame IS the authentication. The
    /// pending nonce for this connection is consumed single-use regardless of
    /// outcome — a missing nonce, a missing sender device, a missing or
    /// unreadable secret, or a bad proof all answer
    /// [`Rejected`](ene_api::v1::handshake::AuthResult::Rejected) with an
    /// operational reason — so a captured proof can never replay. The secret
    /// loads from the file-backed `auth_store` on every call:
    /// there is no cache, so rotations and revocations take effect on the
    /// next authentication. Success answers
    /// [`Accepted`](ene_api::v1::handshake::AuthResult::Accepted)
    /// carrying this connection's table id, which the Client echoes on every
    /// later frame as the auth binding the gate checks; the acceptance (and
    /// its piggybacked presence fact) is the first response on this
    /// connection to reveal the id, while every rejection hides it. Proof
    /// comparison itself runs in constant time inside `ene-credential`.
    pub(super) async fn verify_proof(
        &self,
        frame: &WireFrame,
        proof: &AuthProof,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let nonce = lock_map(&self.pending_nonces).remove(&conn_key(&live.connection_id));
        // Device attribution comes from the connection table (paired moments
        // earlier on this same connection), never from the envelope claim:
        // the proof authenticates the pending pairing the Host recorded, and
        // trusting a Client-supplied device here would let any peer claim
        // any identity. The opaque wire string resolves to its domain record
        // through the store — never by parsing, since projections are
        // unrelated to the domain bytes — and the domain id keys the secret.
        let device = live.paired_device.clone();
        let reason = match (nonce, device) {
            (Some(nonce), Some(device)) => {
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
            (Some(_), None) => Some("unknown device"),
            (None, _) => Some("no pending challenge"),
        };
        match reason {
            None => {
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
                        WirePayload::PresenceAttribution(attribution_to_wire(self, &attribution)),
                    ));
                }
                out
            }
            Some(reason) => vec![outgoing_frame_pre_auth(
                frame,
                live,
                WirePayload::AuthResult(AuthResult::Rejected {
                    reason: reason.to_string(),
                }),
            )],
        }
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
/// both resolved table-side.
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
        move_reason: None,
    }
}

/// Builds a pairing denial frame with an operational reason only.
///
/// A denial predates authentication, so its sender hides the connection id.
fn denied_pairing(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame_pre_auth(
        frame,
        live,
        WirePayload::PairingResult(PairingResult::Denied {
            reason: reason.to_string(),
        }),
    )
}
