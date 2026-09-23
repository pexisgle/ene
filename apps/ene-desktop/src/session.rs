use std::path::Path;
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementView,
    ManagementViewRequest, RationaleOrigin, SETUP_COMPLETE_TARGET, consent_target,
    credential_target,
};
use ene_api::v1::payload::WirePayload;
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

use crate::ui::{DesktopError, request_with_timeout};

pub const SETUP_CREDENTIAL_LABEL: &str = "main";

pub const SETUP_PROVIDER_OPENAI: &str = "openai";
pub const CAPABILITY_DIALOGUE: &str = "dialogue";
pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

/// The GUI's bounded bootstrap budget while a freshly started Host comes up.
/// The launcher and the Client connect share the budget; only the retried
/// error class differs.
pub const BOOTSTRAP_ATTEMPTS: u8 = 80;
pub const BOOTSTRAP_DELAY: Duration = Duration::from_millis(50);
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

    #[must_use]
    pub fn setup_ready(&self) -> bool {
        self.credential_present && self.consent_assigned
    }
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
                if attempts >= BOOTSTRAP_ATTEMPTS {
                    return Err(DesktopError::Client(ClientError::Transport(error)));
                }
                tokio::time::sleep(BOOTSTRAP_DELAY).await;
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

pub async fn fetch_setup_view(client: &mut Client) -> Result<ManagementView, DesktopError> {
    match request_with_timeout(client, setup_view_request(), Duration::from_secs(15)).await? {
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
    let send = request_with_timeout(
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
        Duration::from_secs(15),
    )
    .await?;
    let round = match send {
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
            round
        }
        WirePayload::RoundIntakeOutcome(refusal) => {
            return Err(DesktopError::Unavailable(describe_intake_refusal(
                lang, &refusal,
            )));
        }
        other => {
            return Err(DesktopError::Protocol(format!(
                "expected RoundIntakeOutcome, got {}",
                other.message_type()
            )));
        }
    };
    let mut reply = String::new();
    let mut stream_id = None;
    loop {
        match ask_stream(client).await? {
            WirePayload::TextStreamOpen(open) => stream_id = Some(open.stream),
            WirePayload::TextStreamFrame(frame) => reply.push_str(&frame.delta),
            WirePayload::TextStreamClose(close) => {
                if close.status != StreamClose::Completed {
                    return Err(DesktopError::Protocol(String::from(
                        "stream closed without completion",
                    )));
                }
                break;
            }
            // Protocol-defined interleavings the Client must absorb: state-only
            // facts (`Client::next_frame` already observed them) and an
            // auto-presented backlog summary the explicit UndeliveredRequest
            // path re-presents. A stream turn must not fail on any of them.
            WirePayload::PresenceAttribution(_)
            | WirePayload::UndeliveredResponse(_)
            | WirePayload::BodyStateHint(_) => {}
            other => {
                return Err(DesktopError::Protocol(format!(
                    "unexpected stream {}",
                    other.message_type()
                )));
            }
        }
    }
    Ok(ChatTurn {
        round: round.0,
        stream: stream_id,
        reply,
    })
}

/// Maps an intake refusal to the Owner-facing reason in the Owner's locale.
/// The wire type defines these as distinct domain outcomes, so a hold, a stale
/// round, and a revalidation demand must not read as one another.
fn describe_intake_refusal(lang: &str, outcome: &RoundIntakeOutcomeWire) -> String {
    let ja = lang.eq_ignore_ascii_case("ja");
    let text = |japanese: &str, english: &str| {
        if ja {
            String::from(japanese)
        } else {
            String::from(english)
        }
    };
    match outcome {
        RoundIntakeOutcomeWire::StaleRound { .. } => text(
            "前回の状態が古くなっています。最新の状態を確認して、もう一度お送りください。",
            "The previous state is stale. Review the current state and send again.",
        ),
        RoundIntakeOutcomeWire::HeldForTransition => text(
            "パートナーの状態が切り替わっています。落ち着いてからもう一度お試しください。",
            "A presence change is in progress; try again once it settles.",
        ),
        RoundIntakeOutcomeWire::NeedsRevalidation { .. } => text(
            "送信前に最新の状態を確認してください。",
            "Refresh the current state before sending.",
        ),
        // The accepted variant is handled before this helper is reached.
        RoundIntakeOutcomeWire::AcceptedForRound { .. } => text(
            "送信は受け付けられませんでした。",
            "The message was not accepted.",
        ),
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
    match request_with_timeout(client, payload, Duration::from_secs(15)).await? {
        WirePayload::HistoryResponse(HistoryResponse::Items(items)) => Ok(items),
        WirePayload::HistoryResponse(HistoryResponse::Unavailable) => Err(
            DesktopError::Unavailable(String::from("the timeline could not be read; retry later")),
        ),
        WirePayload::HistoryResponse(HistoryResponse::StaleCompanion) => {
            Err(DesktopError::Unavailable(String::from(
                "the companion projection is stale; re-read presence and retry",
            )))
        }
        WirePayload::HistoryResponse(HistoryResponse::InvalidRequest) => Err(
            DesktopError::Protocol(String::from("history request is unusable")),
        ),
        other => Err(DesktopError::Protocol(format!(
            "expected HistoryResponse, got {}",
            other.message_type()
        ))),
    }
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(Duration::from_secs(15), client.next_frame())
        .await
        .map_err(|_| DesktopError::Transport(String::from("stream timed out")))?
        .map_err(DesktopError::Client)
}
