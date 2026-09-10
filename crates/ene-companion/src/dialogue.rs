//! One dialogue turn: admission, durable append, inference, reply.
//!
//! Wire mapping, presentation intake, and composition stay in the Host;
//! this module owns the companion-side turn order. A turn starts after
//! presentation accepts the input: admission resolves and authorizes the
//! inference premise, the owner row commits durably, the provider call runs
//! under the claimed attempt, and an adopted reply registers with the same
//! atomic append. `ene-inference` owns permission, credential, attempt, and
//! usage ordering behind [`InferenceExecutor`]; this module never sees
//! those types.
//!
//! Durable replay precedes acceptance and stays outside a turn (see
//! [`classify_replay`]): an exact retry answers from the stored marker
//! without opening a turn, while a conflicting reuse rejects just as
//! early.

use ene_inference::{
    Admission, AuthorizedInference, InferenceDispatchOutcome, InferenceExecutor, NotSentReason,
};
use ene_learning::{
    ExperienceCandidate, ExperienceRole, ExperienceSourceKind, ExperienceTurn, FormationDecision,
    LearningInference, LearningInferenceError, LearningRepository, LearningTechnicalError,
    SecretScrubber, SourceRangeRef,
};
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};
use thiserror::Error;

use crate::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionTechnicalError,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, RequestFingerprint, RoundIntentMark,
};

/// One presentation-accepted client input, ready for the companion turn.
#[derive(Clone, PartialEq, Eq)]
pub struct AcceptedDialogueInput {
    pub companion: CompanionId,
    /// Host-issued round the input joined or minted.
    pub round: RawId,
    pub generation: PresenceGeneration,
    /// Owner body text; redacted from [`core::fmt::Debug`].
    pub text: String,
    pub lang: String,
    pub local_id: Option<String>,
    pub command: CommandId,
    pub round_wire: String,
    pub round_intent: RoundIntentMark,
    pub incarnation: Option<(u64, u64)>,
}

impl core::fmt::Debug for AcceptedDialogueInput {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AcceptedDialogueInput")
            .field("companion", &self.companion)
            .field("round", &self.round)
            .field("generation", &self.generation)
            .field("text", &"<redacted>")
            .field("lang", &self.lang)
            .field("local_id", &self.local_id)
            .field("command", &self.command)
            .field("round_wire", &self.round_wire)
            .field("round_intent", &self.round_intent)
            .field("incarnation", &self.incarnation)
            .finish()
    }
}

/// A turn whose owner row committed; dispatch and reply are still pending.
///
/// The Host records the open round between [`begin_turn`] and
/// [`finish_turn`], so nothing in here is inspected outside this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueTurn {
    input: AcceptedDialogueInput,
    authorized: AuthorizedInference,
}

/// Result of starting a turn.
///
/// Replay, stale, conflict, and held variants leave no open-round record
/// with the caller; only [`DialogueBegin::Ready`] carries a turn forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueBegin {
    /// The owner row committed; dispatch is next.
    Ready(Box<DialogueTurn>),
    /// The command already committed: answers the stored accept.
    Replayed {
        round: RawId,
        round_wire: Option<String>,
    },
    StaleExpected {
        /// Current generation the caller should observe next time.
        current: PresenceGeneration,
    },
    /// The expected consent moved underneath the append.
    StaleConsent,
    /// The reused command key owns a different request.
    Conflict,
    /// A store failure held the append.
    Held,
    HeldByLifecycle(CompanionLifecycle),
    /// Admission declined without side effects.
    Declined(NotSentReason),
}

/// Result of finishing a turn.
#[derive(Clone, PartialEq, Eq)]
pub enum DialogueOutcome {
    /// The reply committed; text is ready for the caller's stream.
    Completed {
        /// Adopted reply body; redacted from [`core::fmt::Debug`].
        text: String,
    },
    /// The reply could not be adopted: the caller closes interrupted.
    Interrupted,
}

impl core::fmt::Debug for DialogueOutcome {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Completed { .. } => formatter
                .debug_struct("Completed")
                .field("text", &"<redacted>")
                .finish(),
            Self::Interrupted => formatter.write_str("Interrupted"),
        }
    }
}

/// Durable replay classification for one command-scoped client send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayClassification {
    /// The stored row proves an exact retry: answer its original accept.
    Replay {
        round: RawId,
        round_wire: Option<String>,
    },
    /// The stored row proves a different request under the same key.
    Conflict,
    /// No row carries this key: the send proceeds.
    None,
    /// A store failure held the lookup.
    Held,
}

/// Classifies one command-scoped send against durable history.
///
/// The comparison is the same [`RequestFingerprint`] the store compares
/// in-transaction: a stored row whose fingerprint is absent or different
/// fails closed as [`ReplayClassification::Conflict`].
pub async fn classify_replay(
    history: &impl HistoryRepository,
    companion: CompanionId,
    command: &CommandId,
    incoming: RequestFingerprint,
) -> ReplayClassification {
    match history.lookup_command(companion, command).await {
        Err(_) => ReplayClassification::Held,
        Ok(None) => ReplayClassification::None,
        Ok(Some(found)) => {
            let replays = found
                .request_fingerprint()
                .is_some_and(|stored| stored == incoming);
            if replays {
                ReplayClassification::Replay {
                    round: found.round,
                    round_wire: found.round_wire,
                }
            } else {
                ReplayClassification::Conflict
            }
        }
    }
}

/// Admits one accepted input and commits its owner row durably.
///
/// Admission precedes the append, so a declined input leaves neither a
/// history row nor an open-round record. An append that already committed
/// under the same command resolves the stored row and reports
/// [`DialogueBegin::Replayed`] without dispatching.
pub async fn begin_turn(
    input: AcceptedDialogueInput,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
) -> DialogueBegin {
    let authorized = match inference.admit_dialogue().await {
        Ok(Admission::Admitted(authorized)) => *authorized,
        Ok(Admission::Declined(reason)) => return DialogueBegin::Declined(reason),
        Err(_) => return DialogueBegin::Held,
    };
    let (consent_id, consent_rev) = {
        let (id, rev) = authorized.consent_premise();
        (id.to_owned(), rev)
    };
    let owner = AppendHistoryCommand {
        companion: input.companion,
        round: input.round,
        role: HistoryRole::Owner,
        text: input.text.clone(),
        lang: input.lang.clone(),
        at: WallClockWithTz::now(),
        expected_generation: input.generation,
        expected_consent: Some((consent_id, consent_rev)),
        local_id: input.local_id.clone(),
        command_id: Some(input.command),
        round_wire: Some(input.round_wire.clone()),
        round_intent: Some(input.round_intent.clone()),
        incarnation: input.incarnation,
    };
    match history.append_message(owner).await {
        Ok(HistoryAppendOutcome::CommittedAs { .. }) => {
            DialogueBegin::Ready(Box::new(DialogueTurn { input, authorized }))
        }
        // Lost the race with a concurrent same-command submit after the
        // early replay lookup: resolve the stored row so the ack survives
        // restarts like any other replay, and do not dispatch.
        Ok(HistoryAppendOutcome::AlreadyCommittedAs { .. }) => {
            match history
                .lookup_command(input.companion, &input.command)
                .await
            {
                Ok(Some(found)) => DialogueBegin::Replayed {
                    round: found.round,
                    round_wire: found.round_wire,
                },
                _ => DialogueBegin::Held,
            }
        }
        Ok(HistoryAppendOutcome::StaleExpected { current }) => {
            DialogueBegin::StaleExpected { current }
        }
        Ok(HistoryAppendOutcome::StaleConsent) => DialogueBegin::StaleConsent,
        Ok(HistoryAppendOutcome::CommandConflict) => DialogueBegin::Conflict,
        Ok(HistoryAppendOutcome::HeldByLifecycle { lifecycle }) => {
            DialogueBegin::HeldByLifecycle(lifecycle)
        }
        Err(_) => DialogueBegin::Held,
    }
}

/// Dispatches the turn's inference call and registers an adopted reply.
///
/// A never-sent or technical outcome closes the stream interrupted; usage
/// accounting is already decided inside the inference boundary. An adopted
/// reply appends with its undelivered registration in the same atomic
/// section; any other reply outcome is interrupted.
pub async fn finish_turn(
    turn: Box<DialogueTurn>,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
) -> DialogueOutcome {
    let DialogueTurn { input, authorized } = *turn;
    let (consent_id, consent_rev) = {
        let (id, rev) = authorized.consent_premise();
        (id.to_owned(), rev)
    };
    match inference.dispatch(authorized, input.text.clone()).await {
        Ok(InferenceDispatchOutcome::Completed {
            arrival,
            adopted: true,
        }) => {
            let reply = AppendHistoryCommand {
                companion: input.companion,
                round: input.round,
                role: HistoryRole::Companion,
                text: arrival.output_text.clone(),
                lang: input.lang.clone(),
                at: WallClockWithTz::now(),
                expected_generation: input.generation,
                expected_consent: Some((consent_id, consent_rev)),
                local_id: None,
                command_id: None,
                // Same round, same projection; the reply is Host-produced,
                // so it carries no command key and no round intent.
                round_wire: Some(input.round_wire.clone()),
                round_intent: None,
                incarnation: None,
            };
            match history.append_reply_with_undelivered(reply, true).await {
                Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => DialogueOutcome::Completed {
                    text: arrival.output_text,
                },
                _ => DialogueOutcome::Interrupted,
            }
        }
        Ok(
            InferenceDispatchOutcome::Completed { adopted: false, .. }
            | InferenceDispatchOutcome::NotSent(_),
        )
        | Err(_) => DialogueOutcome::Interrupted,
    }
}

/// Recent History messages read into one Experience source.
pub const EXPERIENCE_SOURCE_MESSAGES: u64 = 12;

/// Why one Experience proposal could not be completed.
///
/// A model answer that cannot be interpreted is the domain outcome
/// [`FormationDecision::DeferredForContext`], not an error.
#[derive(Debug, Error)]
pub enum ExperienceProposalError {
    #[error("history unavailable for experience formation: {0}")]
    History(#[from] CompanionTechnicalError),
    #[error("learning formation failed: {0}")]
    Formation(#[from] LearningTechnicalError),
}

/// Proposes one Experience from the companion's recent History and lets
/// Learning judge it.
///
/// The caller runs this after a reply is durable; the result never changes
/// whether that reply completed. The transcript is transient source material
/// for this formation only, and the Summary records only a source-range
/// reference back to the retained History.
pub async fn propose_experience(
    companion: CompanionId,
    history: &impl HistoryRepository,
    learning: &impl LearningRepository,
    inference: &impl InferenceExecutor,
    scrubber: &impl SecretScrubber,
) -> Result<FormationDecision, ExperienceProposalError> {
    let items = history
        .load_recent_timeline(companion, EXPERIENCE_SOURCE_MESSAGES)
        .await?;
    let (Some(first), Some(last)) = (items.first(), items.last()) else {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    };
    let transcript = items
        .iter()
        .map(|item| ExperienceTurn {
            role: match item.role {
                HistoryRole::Owner => ExperienceRole::Owner,
                HistoryRole::Companion => ExperienceRole::Companion,
            },
            text: item.text.clone(),
        })
        .collect();
    let candidate = ExperienceCandidate {
        companion: companion.as_raw(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: first.id,
            end: last.id,
        },
        transcript,
        at: WallClockWithTz::now(),
    };
    let adapter = LearningInferenceAdapter { inference };
    Ok(ene_learning::form_experience(learning, &adapter, scrubber, candidate).await?)
}

/// Maps the inference owner boundary onto the opaque Learning inference port.
///
/// The learning consumer and purpose are admitted by `ene-inference`; this
/// adapter never presents the call as dialogue.
struct LearningInferenceAdapter<'a, I> {
    inference: &'a I,
}

impl<I: InferenceExecutor + Send + Sync> LearningInference for LearningInferenceAdapter<'_, I> {
    async fn infer(&self, prompt: String) -> Result<String, LearningInferenceError> {
        match self.inference.admit_learning().await {
            Ok(Admission::Admitted(authorized)) => {
                match self.inference.dispatch(*authorized, prompt).await {
                    Ok(InferenceDispatchOutcome::Completed {
                        arrival,
                        adopted: true,
                    }) => Ok(arrival.output_text),
                    Ok(
                        InferenceDispatchOutcome::Completed { adopted: false, .. }
                        | InferenceDispatchOutcome::NotSent(_),
                    )
                    | Err(_) => Err(LearningInferenceError::Declined),
                }
            }
            Ok(Admission::Declined(_)) => Err(LearningInferenceError::Declined),
            Err(error) => Err(LearningInferenceError::Unavailable {
                reason: error.to_string(),
            }),
        }
    }
}
