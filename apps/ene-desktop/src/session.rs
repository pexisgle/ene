//! Client-channel session: pairing, chat, history, setup views.
//!
//! Uses [`ene_client`] and [`ene_api`] only. No DB schema.

use std::path::Path;
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementView,
    ManagementViewRequest, RationaleOrigin, SETUP_COMPLETE_TARGET, consent_target,
    credential_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::PresenceStateWire;
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    StreamWireId, TextLangWire,
};
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryItem, HistoryRequest, HistoryResponse, PresentationStatus,
    RoundIntakeOutcomeWire, StreamClose, SubmitTextInput, TextBodyWire,
};
use ene_client::error::ClientError;
use ene_client::{Client, ConnectProgress, DEFAULT_COMPANION_REF, PendingPairingClient};

use crate::ui::{DesktopError, IntakeRefusal, StreamEnd};

pub const SETUP_CREDENTIAL_LABEL: &str = "main";

/// The Windows Client pipe name of one data directory: the same FNV-1a fold
/// the Host uses, so the GUI's requester listener derivation matches.
#[cfg(any(windows, test))]
#[must_use]
pub fn client_pipe_name(data_dir: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut tag = FNV_OFFSET;
    for byte in data_dir.as_os_str().as_encoded_bytes() {
        tag ^= u64::from(*byte);
        tag = tag.wrapping_mul(FNV_PRIME);
    }
    format!(r"\\.\pipe\ene-{tag:016x}")
}
pub const SETUP_PROVIDER_OPENAI: &str = "openai";
pub const CAPABILITY_DIALOGUE: &str = "dialogue";
pub const DEFAULT_HISTORY_LIMIT: u64 = 50;
const HOST_SETUP_SECTIONS: &[&str] = &["provider", "model", "consent", "credential", "learning"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub round: String,
    pub stream: Option<StreamWireId>,
    pub reply: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupFacts {
    pub credential_present: bool,
    pub consent_assigned: bool,
    pub model: Option<String>,
    pub mark: String,
}

impl SetupFacts {
    #[must_use]
    pub fn from_view(view: &ManagementView) -> Self {
        let mut credential_present = false;
        let mut consent_assigned = false;
        let mut model = None;
        for section in &view.sections {
            match section.kind.as_str() {
                "credential" => credential_present = section.body.starts_with("present"),
                "consent" => consent_assigned = section.body.starts_with("rev "),
                "model" if section.body != "unconfigured" => {
                    model = Some(section.body.clone());
                }
                _ => {}
            }
        }
        Self {
            credential_present,
            consent_assigned,
            model,
            mark: view.mark.0.clone(),
        }
    }

    /// Setup completion is derived from Host facts, never a local wizard flag.
    #[must_use]
    pub fn setup_ready(&self) -> bool {
        self.credential_present && self.consent_assigned
    }
}

pub async fn connect(data_dir: &Path, descriptor: &str) -> Result<Client, ClientError> {
    Client::connect(data_dir, descriptor, &ene_client::platform_display()).await
}

/// One GUI connect attempt: an established session, or a pairing that is
/// still waiting for the Owner.
///
/// A stored device authenticates and returns [`Paired`](Self::Paired); only a
/// first run (or a run whose device file is gone) pends. The two are never
/// conflated: a successful connect is not an error, and a pending pairing owns
/// the connection the GUI must retain until confirmation completes.
pub enum DesktopConnect {
    Paired(Box<Client>),
    PendingOwnerConfirmation(PendingPairingClient),
}

/// Connects the GUI as a Client, or reports the pending pairing that still
/// needs the Owner.
///
/// The Host may be starting, so transport failures retry with a bounded
/// budget before surfacing. A degraded device file, a denied pairing, or a
/// rejected proof stays an explicit error: the first-run provisioning path is
/// never taken for state the Host already refused.
pub async fn connect_or_pending(
    data_dir: &Path,
    descriptor: &str,
) -> Result<DesktopConnect, DesktopError> {
    let mut attempts = 0_u8;
    loop {
        match Client::begin_connect(data_dir, descriptor, &ene_client::platform_display()).await {
            Ok(ConnectProgress::Connected(client)) => {
                return Ok(DesktopConnect::Paired(Box::new(client)));
            }
            Ok(ConnectProgress::Pending(pending)) => {
                return Ok(DesktopConnect::PendingOwnerConfirmation(pending));
            }
            Err(ClientError::Transport(error)) => {
                attempts = attempts.saturating_add(1);
                if attempts >= 80 {
                    return Err(DesktopError::Client(ClientError::Transport(error)));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(DesktopError::Client(error)),
        }
    }
}

pub fn setup_view_request() -> WirePayload {
    WirePayload::ManagementViewRequest(ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
        memory_after: None,
        memory_revisions_of: None,
        memory_revisions_after: None,
    })
}

pub fn credential_intent(mark: &str, provider: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ConfigureCredentialIntent,
        target: credential_target(provider, SETUP_CREDENTIAL_LABEL),
        base_view: BaseViewMark(mark.to_string()),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    })
}

pub fn assignment_intent(mark: &str, provider: &str, model: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: consent_target(
            CAPABILITY_DIALOGUE,
            provider,
            model,
            &format!("{provider}:{SETUP_CREDENTIAL_LABEL}"),
        ),
        base_view: BaseViewMark(mark.to_string()),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    })
}

pub fn setup_complete_intent(mark: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: ManagementTargetWire(String::from(SETUP_COMPLETE_TARGET)),
        base_view: BaseViewMark(mark.to_string()),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    })
}

pub fn confirmed_true_intent(mark: &str) -> WirePayload {
    let mut intent = assignment_intent(mark, SETUP_PROVIDER_OPENAI, "must-not-apply");
    if let WirePayload::ManagementIntent(inner) = &mut intent {
        inner.confirmed = true;
    }
    intent
}

pub async fn fetch_setup_view(client: &mut Client) -> Result<ManagementView, DesktopError> {
    match ask(client, setup_view_request()).await? {
        WirePayload::ManagementView(view) => Ok(view),
        other => Err(DesktopError::Protocol(format!(
            "expected ManagementView, got {}",
            other.message_type()
        ))),
    }
}

pub async fn submit_and_collect(
    client: &mut Client,
    text: &str,
    lang: &str,
) -> Result<ChatTurn, DesktopError> {
    let companion = client.companion_ref();
    let send = ask(
        client,
        WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(companion),
            round: None,
            fresh: false,
            local_id: ClientLocalId(uuid::Uuid::new_v4().to_string()),
            body: TextBodyWire {
                text: text.to_string(),
                lang: TextLangWire(lang.to_string()),
            },
        }),
    )
    .await?;
    let round = intake_round(send)?;
    let mut reply = String::new();
    let mut stream_id = None;
    loop {
        match ask_stream(client).await? {
            WirePayload::TextStreamOpen(open) => stream_id = Some(open.stream),
            WirePayload::TextStreamFrame(frame) => reply.push_str(&frame.delta),
            WirePayload::TextStreamClose(close) => match stream_end(close.status) {
                None => break,
                Some(end) => return Err(DesktopError::StreamIncomplete(end)),
            },
            other => {
                return Err(DesktopError::Protocol(format!(
                    "unexpected stream {}",
                    other.message_type()
                )));
            }
        }
    }
    Ok(ChatTurn {
        round,
        stream: stream_id,
        reply,
    })
}

/// One intake answer: the round to submit into, or the refusal to explain.
/// Host acceptance and the three refusals are matched separately, so a
/// refusal can never be reported as an accepted round and a non-intake
/// payload stays a protocol failure rather than a guessed domain outcome.
fn intake_round(answer: WirePayload) -> Result<String, DesktopError> {
    match answer {
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
            Ok(round.0)
        }
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. }) => {
            Err(DesktopError::IntakeRejected(IntakeRefusal::StaleRound))
        }
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition) => Err(
            DesktopError::IntakeRejected(IntakeRefusal::HeldForTransition),
        ),
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation { .. }) => Err(
            DesktopError::IntakeRejected(IntakeRefusal::NeedsRevalidation),
        ),
        other => Err(DesktopError::Protocol(format!(
            "expected RoundIntakeOutcome, got {}",
            other.message_type()
        ))),
    }
}

/// A non-completed stream end to report, or [`None`] when the turn completed
/// and the collection loop stops successfully.
#[must_use]
fn stream_end(status: StreamClose) -> Option<StreamEnd> {
    match status {
        StreamClose::Completed => None,
        StreamClose::Interrupted => Some(StreamEnd::Interrupted),
        StreamClose::Cancelled => Some(StreamEnd::Cancelled),
        StreamClose::Stale => Some(StreamEnd::Stale),
    }
}

/// Presentation ACK for one collected chat turn. Call only from the path
/// that actually presented that receipt. Mere receive is not
/// [`PresentationStatus::Presented`]. `send_text` ACKs
/// PresentationStatus::Presented only after the timeline shows the turn.
pub async fn confirm_chat_presentation(
    client: &mut Client,
    turn: &ChatTurn,
    status: PresentationStatus,
) -> Result<(), DesktopError> {
    client
        .notify(WirePayload::ConfirmPresentation(ConfirmPresentationWire {
            round: ene_api::v1::refs::RoundWireId(turn.round.clone()),
            stream: turn.stream,
            status,
            detail: None,
        }))
        .await
        .map_err(DesktopError::Client)
}

pub async fn fetch_history(
    client: &mut Client,
    limit: u64,
) -> Result<Vec<HistoryItem>, DesktopError> {
    let companion = client.companion_ref();
    let payload = WirePayload::HistoryRequest(HistoryRequest {
        companion: CompanionWireRef(if companion.is_empty() {
            DEFAULT_COMPANION_REF.to_string()
        } else {
            companion
        }),
        since: None,
        limit,
        round: None,
    });
    match ask(client, payload).await? {
        WirePayload::HistoryResponse(HistoryResponse::Items(items)) => Ok(items),
        WirePayload::HistoryResponse(_) => Err(DesktopError::Protocol(String::from(
            "history was unavailable",
        ))),
        other => Err(DesktopError::Protocol(format!(
            "expected HistoryResponse, got {}",
            other.message_type()
        ))),
    }
}

pub fn presence_label(state: PresenceStateWire) -> &'static str {
    match state {
        PresenceStateWire::Present => "present",
        PresenceStateWire::NoActive => "no-active",
        PresenceStateWire::InTransition => "in-transition",
        PresenceStateWire::Stopped => "stopped",
        PresenceStateWire::RecoveryWait => "recovery-wait",
    }
}

async fn ask(client: &mut Client, payload: WirePayload) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(Duration::from_secs(15), client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(Duration::from_secs(15), client.next_frame())
        .await
        .map_err(|_| DesktopError::Transport(String::from("stream timed out")))?
        .map_err(DesktopError::Client)
}

#[cfg(test)]
mod tests {
    use super::{SetupFacts, intake_round, stream_end};
    use crate::ui::{DesktopError, IntakeRefusal, StreamEnd};
    use ene_api::v1::management::{ManagementView, ViewSection};
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{RevalidationReasonWire, RoundWireId, ViewMarkWire};
    use ene_api::v1::round::{
        ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
    };

    #[test]
    fn setup_ready_requires_host_facts_not_a_local_flag() {
        let view = ManagementView {
            mark: ViewMarkWire(String::from("mark-1")),
            sections: vec![
                ViewSection {
                    kind: String::from("credential"),
                    title: String::from("Credential"),
                    body: String::from("present (memory)"),
                },
                ViewSection {
                    kind: String::from("consent"),
                    title: String::from("Consent"),
                    body: String::from("none"),
                },
                ViewSection {
                    kind: String::from("model"),
                    title: String::from("Model"),
                    body: String::from("unconfigured"),
                },
            ],
        };
        let facts = SetupFacts::from_view(&view);
        assert!(facts.credential_present);
        assert!(!facts.consent_assigned);
        assert!(!facts.setup_ready());
    }

    #[test]
    fn each_intake_refusal_is_reported_as_its_own_domain_outcome() {
        let accepted = WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
            round: RoundWireId(String::from("round-1")),
        });
        let round = intake_round(accepted).expect("Host accepted the round");
        assert_eq!(round, "round-1");

        let refusals = [
            (
                RoundIntakeOutcomeWire::StaleRound {
                    current_round: None,
                    current_generation: 3,
                },
                IntakeRefusal::StaleRound,
            ),
            (
                RoundIntakeOutcomeWire::HeldForTransition,
                IntakeRefusal::HeldForTransition,
            ),
            (
                RoundIntakeOutcomeWire::NeedsRevalidation {
                    reason: RevalidationReasonWire(String::from("stopped-companion")),
                },
                IntakeRefusal::NeedsRevalidation,
            ),
        ];
        for (outcome, expected) in refusals {
            let error = intake_round(WirePayload::RoundIntakeOutcome(outcome))
                .expect_err("a refusal never accepts a round");
            assert!(
                matches!(error, DesktopError::IntakeRejected(refusal) if refusal == expected),
                "the refusal keeps its own remedy, got {error:?}"
            );
        }

        let unrelated = WirePayload::ConfirmPresentation(ConfirmPresentationWire {
            round: RoundWireId(String::from("round-1")),
            stream: None,
            status: PresentationStatus::Presented,
            detail: None,
        });
        let error = intake_round(unrelated)
            .expect_err("a payload that is not an intake outcome cannot accept a round");
        assert!(
            matches!(error, DesktopError::Protocol(_)),
            "an unexpected payload stays a protocol failure, got {error:?}"
        );
    }

    #[test]
    fn a_completed_stream_ends_collection_and_any_other_end_is_reportable() {
        assert_eq!(stream_end(StreamClose::Completed), None);
        assert_eq!(
            stream_end(StreamClose::Interrupted),
            Some(StreamEnd::Interrupted)
        );
        assert_eq!(
            stream_end(StreamClose::Cancelled),
            Some(StreamEnd::Cancelled)
        );
        assert_eq!(stream_end(StreamClose::Stale), Some(StreamEnd::Stale));
    }
}
