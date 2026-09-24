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
    RevalidationReasonWire, StreamWireId, TextLangWire,
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
pub enum ChatIntakeRefusal {
    StaleRound,
    HeldForTransition,
    NeedsRevalidation { reason: RevalidationReasonWire },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStreamEnd {
    Interrupted,
    Cancelled,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatSessionOutcome {
    Completed(ChatTurn),
    Refused(ChatIntakeRefusal),
    StreamEnded { round: String, end: ChatStreamEnd },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatSendReport {
    NoDraft,
    NotConnected,
    Refused(ChatIntakeRefusal),
    Completed,
    StreamEnded(ChatStreamEnd),
    ReplyShownHistoryRefreshFailed,
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
) -> Result<ChatSessionOutcome, DesktopError> {
    let companion = client.companion_ref();
    let target = client.round_target();
    let send = request_with_timeout(
        client,
        WirePayload::SubmitTextInput(SubmitTextInput {
            companion: CompanionWireRef(companion),
            target,
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
            let Some(refusal) = intake_refusal(refusal) else {
                return Err(DesktopError::Protocol(String::from(
                    "accepted intake reached the refusal path",
                )));
            };
            return Ok(ChatSessionOutcome::Refused(refusal));
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
                if let Some(end) = stream_end(close.status) {
                    return Ok(ChatSessionOutcome::StreamEnded {
                        round: round.0.clone(),
                        end,
                    });
                }
                break;
            }
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
    Ok(ChatSessionOutcome::Completed(ChatTurn {
        round: round.0,
        stream: stream_id,
        reply,
    }))
}

fn intake_refusal(outcome: RoundIntakeOutcomeWire) -> Option<ChatIntakeRefusal> {
    match outcome {
        RoundIntakeOutcomeWire::StaleRound { .. } => Some(ChatIntakeRefusal::StaleRound),
        RoundIntakeOutcomeWire::HeldForTransition => Some(ChatIntakeRefusal::HeldForTransition),
        RoundIntakeOutcomeWire::NeedsRevalidation { reason } => {
            Some(ChatIntakeRefusal::NeedsRevalidation { reason })
        }
        RoundIntakeOutcomeWire::AcceptedForRound { .. } => None,
    }
}

#[must_use]
fn stream_end(status: StreamClose) -> Option<ChatStreamEnd> {
    match status {
        StreamClose::Completed => None,
        StreamClose::Interrupted => Some(ChatStreamEnd::Interrupted),
        StreamClose::Cancelled => Some(ChatStreamEnd::Cancelled),
        StreamClose::Stale => Some(ChatStreamEnd::Stale),
    }
}

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

#[cfg(test)]
mod tests {
    use super::{ChatIntakeRefusal, ChatStreamEnd, intake_refusal, stream_end};
    use ene_api::v1::refs::RevalidationReasonWire;
    use ene_api::v1::round::{RoundIntakeOutcomeWire, StreamClose};

    #[test]
    fn non_completed_stream_statuses_remain_distinct() {
        assert_eq!(stream_end(StreamClose::Completed), None);
        assert_eq!(
            stream_end(StreamClose::Interrupted),
            Some(ChatStreamEnd::Interrupted)
        );
        assert_eq!(
            stream_end(StreamClose::Cancelled),
            Some(ChatStreamEnd::Cancelled)
        );
        assert_eq!(stream_end(StreamClose::Stale), Some(ChatStreamEnd::Stale));
    }

    #[test]
    fn intake_refusal_preserves_the_revalidation_reason() {
        let reason = RevalidationReasonWire(String::from("input-over-limit"));
        let mapped = intake_refusal(RoundIntakeOutcomeWire::NeedsRevalidation {
            reason: reason.clone(),
        });
        assert_eq!(
            mapped,
            Some(ChatIntakeRefusal::NeedsRevalidation { reason })
        );
        assert_eq!(
            intake_refusal(RoundIntakeOutcomeWire::StaleRound {
                current_round: None,
                current_generation: 1,
            }),
            Some(ChatIntakeRefusal::StaleRound)
        );
        assert_eq!(
            intake_refusal(RoundIntakeOutcomeWire::HeldForTransition),
            Some(ChatIntakeRefusal::HeldForTransition)
        );
    }
}
