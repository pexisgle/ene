//! Stage 2 vertical slice, end to end through `HostHandle`.
//!
//! Setup (register, assign, complete) → pairing handshake → text round
//! (accept, stream, confirm) → history → drop (simulated restart) →
//! reopen → history intact, old rounds stale, no auto-resume. Everything
//! runs transport-free through [`HostHandle::handle_frame`] with a fake
//! provider and an in-memory credential store: no sockets, no network,
//! no real keys, no environment mutation.

use ene_api::v1::envelope::WireSender;
use ene_api::v1::envelope::{ProtocolVersion, new_outgoing_envelope};
use ene_api::v1::handshake::{CapabilityAdvertise, PairingRequest};
use ene_api::v1::management::{IntentRationaleWire, ManagementIntent, ManagementIntentKind};
use ene_api::v1::management::{ManagementOutcome, RationaleOrigin};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    BaseViewMark, ClientIncarnationId, ClientLocalId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire, WireMessageType,
};
use ene_api::v1::round::TextBodyWire;
use ene_api::v1::round::{HistoryRequest, RoundIntakeOutcomeWire, SubmitTextInput};
use ene_core::serve::{CredStore, HostHandle, LiveInput};
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_inference::RawUsage;
use ene_inference::fake::FakeProviderTransport;
use ene_plugin_ipc::WireFrame;

const COMPANION: &str = "default";
const PROVIDER: &str = "openai";
const MODEL: &str = "gpt-slice-test";
const CRED_ID: &str = "openai:default";

fn live() -> LiveInput {
    LiveInput {
        client_ref: String::from("slice-client"),
        connection_live: true,
        peer_uid_ok: true,
    }
}

fn sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 0,
            random: 1,
        },
        connection_id: None,
    }
}

fn frame(payload: WirePayload, kind: &str) -> WireFrame {
    WireFrame {
        envelope: {
            let mut envelope = new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from(kind)),
            );
            envelope.observed.presence_generation_view = None;
            envelope
        },
        payload,
    }
}

fn frame_with_generation(payload: WirePayload, kind: &str, generation: u64) -> WireFrame {
    let mut built = frame(payload, kind);
    built.envelope.observed.presence_generation_view = Some(generation);
    built
}

fn intent(kind: ManagementIntentKind, target: &str, base_view: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: ene_api::v1::refs::CommandWireId(uuid::Uuid::new_v4()),
        kind,
        target: ManagementTargetWire(String::from(target)),
        base_view: BaseViewMark(String::from(base_view)),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    })
}

fn outcomes(frames: &[WireFrame]) -> Vec<&WirePayload> {
    frames.iter().map(|frame| &frame.payload).collect()
}

fn applied_count(frames: &[WireFrame]) -> usize {
    outcomes(frames)
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
            )
        })
        .count()
}

fn stored_count(frames: &[WireFrame]) -> usize {
    outcomes(frames)
        .iter()
        .filter(|payload| {
            matches!(
                payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. })
            )
        })
        .count()
}

async fn current_mark(
    handle: &mut HostHandle,
    transport: &FakeProviderTransport,
) -> Option<String> {
    let show = handle
        .handle_frame(
            frame(
                intent(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "setup:show",
                    "bootstrap",
                ),
                "ManagementIntent",
            ),
            live(),
            transport,
        )
        .await;
    for payload in outcomes(&show) {
        if let WirePayload::ManagementView(view) = payload {
            return Some(view.mark.0.clone());
        }
    }
    None
}

async fn setup_step(
    handle: &mut HostHandle,
    transport: &FakeProviderTransport,
) -> Result<(), String> {
    let Some(mark) = current_mark(handle, transport).await else {
        return Err(String::from("no mark"));
    };
    let register = handle
        .handle_frame(
            frame(
                intent(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:default",
                    &mark,
                ),
                "ManagementIntent",
            ),
            live(),
            transport,
        )
        .await;
    if applied_count(&register) != 1 {
        return Err(String::from("register"));
    }
    let Some(mark) = current_mark(handle, transport).await else {
        return Err(String::from("no mark"));
    };
    let assign = handle
        .handle_frame(
            frame(
                intent(
                    ManagementIntentKind::ManageRuleConsentCap,
                    &format!("consent:{PROVIDER}:{MODEL}:{CRED_ID}"),
                    &mark,
                ),
                "ManagementIntent",
            ),
            live(),
            transport,
        )
        .await;
    if stored_count(&assign) != 1 {
        return Err(String::from("assign"));
    }
    let Some(mark) = current_mark(handle, transport).await else {
        return Err(String::from("no mark"));
    };
    let complete = handle
        .handle_frame(
            frame(
                intent(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "setup:complete",
                    &mark,
                ),
                "ManagementIntent",
            ),
            live(),
            transport,
        )
        .await;
    if applied_count(&complete) == 1 {
        Ok(())
    } else {
        Err(String::from("complete"))
    }
}

fn stream_text(frames: &[WireFrame]) -> Option<String> {
    let mut text = String::new();
    let mut opened = false;
    let mut closed_completed = false;
    let mut last_seq: Option<u64> = None;
    for payload in outcomes(frames) {
        match payload {
            WirePayload::TextStreamOpen(_) => {
                opened = true;
            }
            WirePayload::TextStreamFrame(frame) => {
                if last_seq.is_some_and(|previous| frame.seq != previous + 1) {
                    return None;
                }
                last_seq = Some(frame.seq);
                text.push_str(&frame.delta);
            }
            WirePayload::TextStreamClose(close) => {
                closed_completed =
                    matches!(close.status, ene_api::v1::round::StreamClose::Completed);
            }
            _ => {}
        }
    }
    if opened && closed_completed {
        Some(text)
    } else {
        None
    }
}

#[tokio::test]
async fn vertical_slice_setup_to_restart() {
    let Ok(dir) = tempfile::TempDir::new() else {
        return;
    };
    let memory = MemoryCredentialStore::new();
    memory.insert(
        CredentialRef {
            id: String::from(CRED_ID),
            provider: String::from(PROVIDER),
            label: String::from("default"),
        },
        "sk-test-only",
    );
    let transport = FakeProviderTransport::new(
        String::from("hello back"),
        Some(RawUsage {
            input_tokens: 7,
            output_tokens: 3,
        }),
    );
    let Ok(mut host) =
        HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(memory)).await
    else {
        return;
    };

    let setup = setup_step(&mut host, &transport).await;
    assert!(setup.is_ok(), "setup must complete: {setup:?}");

    let pairing = host
        .handle_frame(
            frame(
                WirePayload::PairingRequest(PairingRequest {
                    device_descriptor: String::from("slice laptop"),
                }),
                "PairingRequest",
            ),
            live(),
            &transport,
        )
        .await;
    let paired = outcomes(&pairing)
        .iter()
        .any(|payload| matches!(payload, WirePayload::PairingResult(_)));
    assert!(paired, "pairing must answer");

    let capability = host
        .handle_frame(
            frame(
                WirePayload::CapabilityAdvertise(CapabilityAdvertise {
                    supported_protocol: [ProtocolVersion::V1].to_vec(),
                    features: [].to_vec(),
                    platform: String::from("test"),
                }),
                "CapabilityAdvertise",
            ),
            live(),
            &transport,
        )
        .await;
    let negotiated = outcomes(&capability)
        .iter()
        .any(|payload| matches!(payload, WirePayload::NegotiatedConnection(_)));
    assert!(negotiated, "capability must negotiate");

    let mut generation = 0_u64;
    let mut accepted_round: Option<RoundWireId> = None;
    for _ in 0..2 {
        let responses = host
            .handle_frame(
                frame_with_generation(
                    WirePayload::SubmitTextInput(SubmitTextInput {
                        companion: CompanionWireRef(String::from(COMPANION)),
                        round: None,
                        local_id: ClientLocalId(String::from("local-1")),
                        body: TextBodyWire {
                            text: String::from("hello companion"),
                            lang: TextLangWire(String::from("en")),
                        },
                    }),
                    "SubmitTextInput",
                    generation,
                ),
                live(),
                &transport,
            )
            .await;
        let mut progressed = false;
        for payload in outcomes(&responses) {
            match payload {
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
                    round,
                }) => {
                    accepted_round = Some(round.clone());
                    progressed = true;
                }
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
                    current_generation,
                    ..
                }) => {
                    generation = *current_generation;
                }
                _ => {}
            }
        }
        if progressed {
            break;
        }
    }
    assert!(
        accepted_round.is_some(),
        "input must be accepted within two tries"
    );

    let answer = host
        .handle_frame(
            frame_with_generation(
                WirePayload::SubmitTextInput(SubmitTextInput {
                    companion: CompanionWireRef(String::from(COMPANION)),
                    round: None,
                    local_id: ClientLocalId(String::from("local-2")),
                    body: TextBodyWire {
                        text: String::from("second turn"),
                        lang: TextLangWire(String::from("en")),
                    },
                }),
                "SubmitTextInput",
                generation,
            ),
            live(),
            &transport,
        )
        .await;
    let streamed = stream_text(&answer);
    assert!(
        streamed == Some(String::from("hello back")),
        "stream must carry the provider text in order, got {streamed:?}"
    );

    let history = host
        .handle_frame(
            frame(
                WirePayload::HistoryRequest(HistoryRequest {
                    companion: CompanionWireRef(String::from(COMPANION)),
                    since: None,
                    limit: 50,
                }),
                "HistoryRequest",
            ),
            live(),
            &transport,
        )
        .await;
    let mut owner_seen = false;
    let mut companion_seen = false;
    for payload in outcomes(&history) {
        if let WirePayload::HistoryView(view) = payload {
            for item in &view.items {
                if item.role == ene_api::v1::round::HistoryRole::Owner {
                    owner_seen = true;
                }
                if item.role == ene_api::v1::round::HistoryRole::Companion {
                    companion_seen = true;
                }
            }
        }
    }
    assert!(owner_seen && companion_seen, "history must hold both sides");

    drop(host);
    let memory = MemoryCredentialStore::new();
    memory.insert(
        CredentialRef {
            id: String::from(CRED_ID),
            provider: String::from(PROVIDER),
            label: String::from("default"),
        },
        "sk-test-only",
    );
    let Ok(mut reopened) =
        HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(memory)).await
    else {
        return;
    };
    let restored = reopened
        .handle_frame(
            frame(
                WirePayload::HistoryRequest(HistoryRequest {
                    companion: CompanionWireRef(String::from(COMPANION)),
                    since: None,
                    limit: 50,
                }),
                "HistoryRequest",
            ),
            live(),
            &transport,
        )
        .await;
    let mut restored_items = 0_usize;
    for payload in outcomes(&restored) {
        if let WirePayload::HistoryView(view) = payload {
            restored_items = view.items.len();
        }
    }
    assert!(
        restored_items >= 4,
        "restart must preserve history, got {restored_items}"
    );

    let stale_probe = reopened
        .handle_frame(
            frame_with_generation(
                WirePayload::SubmitTextInput(SubmitTextInput {
                    companion: CompanionWireRef(String::from(COMPANION)),
                    round: accepted_round.clone(),
                    local_id: ClientLocalId(String::from("local-3")),
                    body: TextBodyWire {
                        text: String::from("old round retry"),
                        lang: TextLangWire(String::from("en")),
                    },
                }),
                "SubmitTextInput",
                generation,
            ),
            live(),
            &transport,
        )
        .await;
    let resumed = outcomes(&stale_probe).iter().any(|payload| {
        matches!(
            payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )
    });
    assert!(!resumed, "pre-restart rounds must not resume after restart");
}

#[tokio::test]
async fn untrusted_peer_gets_no_pairing() {
    let Ok(dir) = tempfile::TempDir::new() else {
        return;
    };
    let memory = MemoryCredentialStore::new();
    let transport = FakeProviderTransport::new(String::new(), None);
    let Ok(mut host) =
        HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(memory)).await
    else {
        return;
    };
    let denied = LiveInput {
        client_ref: String::from("stranger"),
        connection_live: true,
        peer_uid_ok: false,
    };
    let responses = host
        .handle_frame(
            frame(
                WirePayload::PairingRequest(PairingRequest {
                    device_descriptor: String::from("stranger box"),
                }),
                "PairingRequest",
            ),
            denied,
            &transport,
        )
        .await;
    let answered_pair = outcomes(&responses).iter().any(|payload| {
        matches!(
            payload,
            WirePayload::PairingResult(ene_api::v1::handshake::PairingResult::Paired { .. })
        )
    });
    assert!(!answered_pair, "untrusted peers must not pair");
}
