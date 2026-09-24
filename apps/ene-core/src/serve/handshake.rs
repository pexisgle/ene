use super::frames::{
    incompatible_protocol, invalid_phase_reject, outgoing_frame, outgoing_frame_pre_auth,
    stale_reject, unpaired_close,
};
use super::{HostHandle, LiveInput, device_client};
use crate::conn::{ChallengeOutcome, ConnectionPhase, InstallOutcome, NonceAdmission};
use crate::pairing_delivery::PendingResend;
use ene_api::v1::envelope::ProtocolVersion;
use ene_api::v1::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, NegotiatedConnection,
    PairingRequest, PairingResult,
};
use ene_api::v1::payload::WirePayload;
use ene_companion::CompanionRepository;
use ene_credential::DevicePairingRepository;
use ene_plugin_ipc::WireFrame;
use ene_presence::PresenceRepository;
use uuid::Uuid;

impl HostHandle {
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
        if let Some(resend) = self.pairing_deliveries.resend_match(
            &live.connection_id,
            frame.envelope.correlation.request_id,
            &descriptor,
        ) {
            return match resend {
                PendingResend::Answer(pending_id) => vec![outgoing_frame_pre_auth(
                    frame,
                    live,
                    WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation {
                        pending_id,
                    }),
                )],
                PendingResend::Conflicting => vec![denied_pairing(
                    frame,
                    live,
                    "pairing request id reused with a different body",
                )],
            };
        }
        let origin = live.connection_id.0.as_hyphenated().to_string();
        match DevicePairingRepository::request_pairing(&self.store, descriptor.clone(), origin)
            .await
        {
            Ok(pending) => {
                if !self.pairing_deliveries.bind_pending(
                    &live.connection_id,
                    &pending.pending_id,
                    frame.envelope.correlation.request_id,
                    &descriptor,
                ) {
                    return vec![denied_pairing(
                        frame,
                        live,
                        "originating pairing connection unavailable",
                    )];
                }
                vec![outgoing_frame_pre_auth(
                    frame,
                    live,
                    WirePayload::PairingResult(PairingResult::PendingOwnerConfirmation {
                        pending_id: pending.pending_id,
                    }),
                )]
            }
            Err(_) => vec![denied_pairing(frame, live, "pairing store unavailable")],
        }
    }

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
            let client_max = advertise
                .supported_protocol
                .iter()
                .copied()
                .max_by_key(|version| (version.major, version.minor))
                .unwrap_or(frame.envelope.protocol);
            return vec![incompatible_protocol(&frame.envelope, live, client_max)];
        }
        let claimed = frame
            .envelope
            .sender
            .device_id
            .as_ref()
            .map(|id| id.0.as_hyphenated().to_string());
        let bind_device = match (&live.paired_device, claimed) {
            (None, Some(claim)) => {
                match DevicePairingRepository::find_device_by_wire(&self.store, &claim).await {
                    Ok(Some(_)) => Some(claim),
                    Ok(None) => return vec![unpaired_close(frame, live)],
                    Err(_) => return Vec::new(),
                }
            }
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

    pub(super) async fn verify_proof(
        &self,
        frame: &WireFrame,
        proof: &AuthProof,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let stored = match live.paired_device.clone() {
            Some(device) => {
                match DevicePairingRepository::find_device_by_wire(&self.store, &device).await {
                    Ok(stored) => stored,
                    Err(_) => return Vec::new(),
                }
            }
            None => None,
        };
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
        let reason = match live.paired_device.clone() {
            Some(_) => {
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
                InstallOutcome::Installed { superseded } => {
                    if let Some(previous) = superseded {
                        self.on_connection_superseded(&previous);
                    }
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
                        if let Some(device) = live.paired_device.clone()
                            && attribution.active_client == Some(device_client(&device))
                        {
                            for summary in self
                                .auto_present_for(frame, live, companion, &attribution)
                                .await
                            {
                                out.push(summary);
                            }
                        }
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

pub(crate) fn attribution_to_wire(
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
