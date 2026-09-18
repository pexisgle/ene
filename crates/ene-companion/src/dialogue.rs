//! One dialogue turn: admission, durable append, inference, reply.
//!
//! Wire mapping, presentation intake, and composition stay in the Host;
//! this module owns the companion-side turn order. A turn starts after
//! presentation accepts the input: [`assemble_dialogue_input`] reads the
//! bounded recent History and recall, admission resolves and authorizes the
//! inference premise carrying that read-set, the owner row commits durably,
//! the provider call runs under the claimed attempt, and an adopted reply
//! registers with the same atomic append. `ene-inference` owns permission,
//! credential, attempt, and usage ordering behind [`InferenceExecutor`]; this
//! module never sees those types.
//!
//! Durable replay precedes acceptance and stays outside a turn (see
//! [`classify_replay`]): an exact retry answers from the stored marker
//! without opening a turn, while a conflicting reuse rejects just as
//! early.
//!
//! Task control follows the same caller-proposes split: [`propose_task`] and
//! [`propose_steering`] map accepted conversation commands onto the Task
//! owner's value premises and return the owner's outcomes unchanged. Adoption
//! decisions and identities stay with `ene-task`, and [`TaskReport`] renders
//! user-facing facts the composition root read from the durable owners.
//!
//! [`finish_turn`] also interprets the companion's own provider output for
//! one closed-world [`DialogueTaskCommand`] (`[task-control] {json}`, final
//! line): the companion-owned interpretation reaches the Task owner only
//! through the composition root's [`DialogueTaskControlPort`], the directive
//! line is never stored, and the stored reply is the owner-derived text. A
//! malformed directive clarifies without changing anything, and a technical
//! failure closes the stream interrupted instead of storing a fabricated
//! reply.

use ene_credential::{CredentialSetRevision, ScrubbedText};
use ene_inference::{
    Admission, AuthorizedInference, DeltaFlow, DeltaSink, DiscardSink, InferenceDispatchOutcome,
    InferenceExecutor, NotSentReason,
};
use ene_learning::{
    ExperienceCandidate, ExperienceRole, ExperienceSourceKind, ExperienceTurn, FormationDecision,
    LearningInference, LearningInferenceError, LearningRepository, LearningTechnicalError,
    RecallQuery, SecretScrubError, SecretScrubber, SourceRangeRef,
};
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, ConversationTaskRepository, OwnerMessageCurrentness, SteeringPremiseRef,
    SteeringProposalPremise, TaskContextOrigin, TaskCreationOutcome, TaskProgress,
    TaskProposalOutcome, TaskProposalPremise, TaskPurpose, TaskRepository, TaskTechnicalError,
    WorkspaceNeedRef, orchestrate_steering_current, orchestrate_task_creation,
    orchestrate_task_creation_current,
};

use crate::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionTechnicalError,
    HistoryAppendOutcome, HistoryMessage, HistoryRepository, HistoryRole, RequestFingerprint,
    RoundIntentMark,
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
    /// Credential-set premise the text was scrubbed under; the owner append
    /// is refused if the set moved.
    pub credential_set: CredentialSetRevision,
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
            .field("credential_set", &self.credential_set)
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
///
/// `prompt` is assembled before admission (the prompt's read-set rides the
/// admission as the attempt's `data_use`) and carried here so the exact bytes
/// admitted are the bytes dispatched; the turn never re-reads History or
/// Memory after its claim.
#[derive(Clone, PartialEq, Eq)]
pub struct DialogueTurn {
    input: AcceptedDialogueInput,
    /// Durable identity of the owner row this turn committed.
    message: RawId,
    prompt: DialogueInput,
    authorized: AuthorizedInference,
}

impl core::fmt::Debug for DialogueTurn {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DialogueTurn")
            .field("input", &self.input)
            .field("message", &self.message)
            .field("prompt", &"<redacted>")
            .field("authorized", &self.authorized)
            .finish()
    }
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
    /// The credential set moved past the input's scrub premise. The input
    /// was not appended; the caller retries so the Host re-scrubs.
    StaleCredentialSet,
    /// The reused command key owns a different request.
    Conflict,
    /// A store failure held the append.
    Held,
    /// A canonical current erasure condition covers the input body
    /// (lifecycle §7/§11). The input was not appended and no undelivered
    /// entry, round, or inference was created; the caller answers a
    /// retry-later hold. Distinct from [`Self::Held`] (a store failure) and
    /// [`Self::StaleCredentialSet`] (the scrub premise moved).
    HeldForErasure,
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
        /// Accepted input of this turn, so the caller can occupy the pin
        /// window and then [`pin_experience`] after the durable reply.
        input: Box<AcceptedDialogueInput>,
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
                .field("input", &"<accepted>")
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
/// history row nor an open-round record. The pre-assembled `prompt` supplies
/// the admission's canonical read-set, so the claim compares exactly the
/// sources the provider input was built from. An append that already
/// committed under the same command resolves the stored row and reports
/// [`DialogueBegin::Replayed`] without dispatching.
pub async fn begin_turn(
    input: AcceptedDialogueInput,
    prompt: DialogueInput,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
) -> DialogueBegin {
    let authorized = match inference.admit_dialogue(prompt.data_use().to_vec()).await {
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
        expected_credential_set: Some(input.credential_set),
        // Owner appends establish recency; only replies answer it.
        expected_owner_message: None,
        local_id: input.local_id.clone(),
        command_id: Some(input.command),
        round_wire: Some(input.round_wire.clone()),
        round_intent: Some(input.round_intent.clone()),
        incarnation: input.incarnation,
    };
    match history.append_message(owner).await {
        Ok(HistoryAppendOutcome::CommittedAs { message }) => {
            DialogueBegin::Ready(Box::new(DialogueTurn {
                input,
                message,
                prompt,
                authorized,
            }))
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
        Ok(HistoryAppendOutcome::StaleCredentialSet) => DialogueBegin::StaleCredentialSet,
        // Owner appends carry no Owner-message premise, so the check is
        // skipped and this arm is unreachable; Held is the safe mapping —
        // retry-safe, with no side effects either way.
        Ok(HistoryAppendOutcome::StaleOwnerInput) => DialogueBegin::Held,
        Ok(HistoryAppendOutcome::CommandConflict) => DialogueBegin::Conflict,
        Ok(HistoryAppendOutcome::HeldForErasure) => DialogueBegin::HeldForErasure,
        Ok(HistoryAppendOutcome::HeldByLifecycle { lifecycle }) => {
            DialogueBegin::HeldByLifecycle(lifecycle)
        }
        Err(_) => DialogueBegin::Held,
    }
}

/// [`begin_turn`] with the durable Owner append supplied by the caller.
///
/// The Host runs the Client-dependent admission (CCT §10.4) as a guarded
/// synchronous section: `commit` executes inside the connection-ownership
/// section through the store's sync append, so a supersession that wins the
/// section cannot leave an Owner row behind, and `lookup` resolves a
/// concurrent same-command commit without leaving the section. This function
/// is synchronous by construction — it never awaits — so the caller can run
/// it on the blocking pool while holding the connection table. Every
/// outcome maps exactly like [`begin_turn`].
pub fn begin_turn_committed<C, L>(
    input: AcceptedDialogueInput,
    prompt: DialogueInput,
    authorized: AuthorizedInference,
    commit: C,
    lookup: L,
) -> DialogueBegin
where
    C: FnOnce(AppendHistoryCommand) -> Result<HistoryAppendOutcome, CompanionTechnicalError>,
    L: FnOnce(CompanionId, &CommandId) -> Result<Option<HistoryMessage>, CompanionTechnicalError>,
{
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
        expected_credential_set: Some(input.credential_set),
        // Owner appends establish recency; only replies answer it.
        expected_owner_message: None,
        local_id: input.local_id.clone(),
        command_id: Some(input.command),
        round_wire: Some(input.round_wire.clone()),
        round_intent: Some(input.round_intent.clone()),
        incarnation: input.incarnation,
    };
    match commit(owner) {
        Ok(HistoryAppendOutcome::CommittedAs { message }) => {
            DialogueBegin::Ready(Box::new(DialogueTurn {
                input,
                message,
                prompt,
                authorized,
            }))
        }
        Ok(HistoryAppendOutcome::AlreadyCommittedAs { .. }) => {
            match lookup(input.companion, &input.command) {
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
        Ok(HistoryAppendOutcome::StaleCredentialSet) => DialogueBegin::StaleCredentialSet,
        // Owner appends carry no Owner-message premise, so the check is
        // skipped and this arm is unreachable; Held is the safe mapping —
        // retry-safe, with no side effects either way.
        Ok(HistoryAppendOutcome::StaleOwnerInput) => DialogueBegin::Held,
        Ok(HistoryAppendOutcome::CommandConflict) => DialogueBegin::Conflict,
        Ok(HistoryAppendOutcome::HeldForErasure) => DialogueBegin::HeldForErasure,
        Ok(HistoryAppendOutcome::HeldByLifecycle { lifecycle }) => {
            DialogueBegin::HeldByLifecycle(lifecycle)
        }
        Err(_) => DialogueBegin::Held,
    }
}

/// Dispatches the turn's already-assembled inference call and registers an
/// adopted reply.
///
/// The prompt was built by [`assemble_dialogue_input`] before admission from
/// bounded recent History and the memories recall offers (retrieval is
/// derived and best-effort: a history or recall read failure degrades to less
/// context rather than failing a reply, and a suppressed Memory is simply
/// absent), and it is carried by the turn unchanged. A secret-boundary
/// failure is not degraded: the owner input and the provider output both pass
/// through the scrubber before they reach a model or durable History, and an
/// unprovable boundary closes the stream interrupted instead of sending or
/// storing raw text. The dispatch carries the prompt's credential-set
/// premise, so the send claim refuses a prompt that predates a credential
/// registration. A never-sent or technical outcome closes the stream
/// interrupted; usage accounting is already decided inside the inference
/// boundary. An adopted reply appends with its undelivered registration in
/// the same atomic section; any other reply outcome is interrupted. Provider
/// deltas are pushed to `sink` as they arrive, each gated on a current
/// presentation premise; a delta shown before an invalidation stays as
/// historical partial presentation, never rewound. `is_current` runs once
/// more after provider completion as an early, best-effort refusal of a
/// superseded reply: it only avoids a doomed append attempt. Durable adoption
/// authority stays inside the append transaction — the reply carries the
/// turn's Owner message identity as its premise, and the store refuses the
/// append when a newer accepted Owner input committed first, even inside
/// the same round, or when the provider claim it was produced under is
/// already associated with a deletion interval. After the durable append, the
/// Experience premise is pinned for the post-response Learning pass. A reply
/// carrying the reserved
/// `[task-control]` protocol is interpreted before the append: a valid
/// first-line-only command runs through the composition root's port and the
/// stored reply is the scrubbed owner outcome, while a marker that is not a
/// valid first-line command is a reserved-protocol violation — no command
/// executes, no reply is stored, and the stream closes interrupted. A
/// technical failure (`Unavailable`) closes interrupted instead of storing a
/// reply no operation produced.
/// The clarification shown for a malformed task-control directive.
const TASK_CONTROL_CLARIFICATION: &str =
    "I could not interpret the task instruction; nothing was changed.";

/// How the presentation sink classified one provider reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlPresentation {
    /// No marker anywhere: ordinary conversation.
    Ordinary,
    /// The first non-whitespace content is the marker: a directive candidate
    /// for the parser to validate.
    Directive,
    /// The marker appeared after ordinary text: reserved-protocol violation,
    /// fail closed.
    LateMarker,
}

/// The sink's marker-detection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlMode {
    /// No non-whitespace content yet.
    Undecided,
    /// Ordinary reply; rolling marker detection.
    Ordinary,
    /// The marker is at the reply start; suppress the rest.
    Directive,
    /// A marker appeared after ordinary text; suppress the rest and fail
    /// closed.
    LateMarker,
}

/// Presentation sink that never lets the reserved `[task-control]` protocol
/// reach the user-visible stream.
///
/// Ordinary text streams as it arrives, with a rolling hold of only the
/// trailing suffix that could still become the marker (at most
/// `TASK_CONTROL_MARKER.len() - 1` bytes), so a marker split across provider
/// deltas is still detected and suppressed. A marker at the first
/// non-whitespace position switches to directive mode; a marker anywhere else
/// switches to late-marker mode and the caller closes the turn interrupted.
/// Directive turns present only the scrubbed typed owner outcome.
struct ControlHoldingSink<'a> {
    inner: Option<&'a mut (dyn DeltaSink + Send)>,
    /// Undecided: all buffered text. Ordinary: the held marker-prefix tail.
    buffer: String,
    mode: ControlMode,
}

impl<'a> ControlHoldingSink<'a> {
    fn new(inner: &'a mut (dyn DeltaSink + Send)) -> Self {
        Self {
            inner: Some(inner),
            buffer: String::new(),
            mode: ControlMode::Undecided,
        }
    }

    async fn push_raw(&mut self, text: &str) -> DeltaFlow {
        let Some(inner) = self.inner.as_mut() else {
            return DeltaFlow::Continue;
        };
        inner.push_delta(text).await
    }

    /// Publishes ordinary text, holding only a trailing suffix that could
    /// still become the marker.
    async fn flush_ordinary(&mut self) -> DeltaFlow {
        if let Some(position) = self.buffer.find(TASK_CONTROL_MARKER) {
            // A marker appeared after ordinary text: present what preceded it
            // and suppress from here on. The caller closes interrupted, so
            // nothing of the malformed reply is stored.
            let prefix = self.buffer[..position].to_owned();
            self.buffer.clear();
            self.mode = ControlMode::LateMarker;
            if prefix.is_empty() {
                return DeltaFlow::Continue;
            }
            return self.push_raw(&prefix).await;
        }
        // Hold the longest suffix that is a proper prefix of the marker.
        let max_hold = (TASK_CONTROL_MARKER.len() - 1).min(self.buffer.len());
        let mut hold = 0;
        for length in (1..=max_hold).rev() {
            let start = self.buffer.len() - length;
            if self.buffer.is_char_boundary(start)
                && TASK_CONTROL_MARKER.starts_with(&self.buffer[start..])
            {
                hold = length;
                break;
            }
        }
        let publish_len = self.buffer.len() - hold;
        if publish_len == 0 {
            return DeltaFlow::Continue;
        }
        let publish: String = self.buffer.drain(..publish_len).collect();
        self.push_raw(&publish).await
    }

    /// Classifies the buffered undecided prefix, switching modes when enough
    /// text is known.
    async fn decide_undecided(&mut self) -> DeltaFlow {
        let Some(start) = self
            .buffer
            .find(|character: char| !character.is_whitespace())
        else {
            return DeltaFlow::Continue;
        };
        if let Some(position) = self.buffer.find(TASK_CONTROL_MARKER) {
            // The marker is reserved: at the first position it is a directive
            // candidate, anywhere else it is a fail-closed violation.
            if position == start {
                self.mode = ControlMode::Directive;
            } else {
                self.mode = ControlMode::LateMarker;
            }
            self.buffer.clear();
            return DeltaFlow::Continue;
        }
        let candidate = &self.buffer[start..];
        if candidate.len() < TASK_CONTROL_MARKER.len() && TASK_CONTROL_MARKER.starts_with(candidate)
        {
            return DeltaFlow::Continue;
        }
        self.mode = ControlMode::Ordinary;
        self.flush_ordinary().await
    }

    async fn push(&mut self, delta: &str) -> DeltaFlow {
        match self.mode {
            ControlMode::Directive | ControlMode::LateMarker => DeltaFlow::Continue,
            ControlMode::Ordinary => {
                self.buffer.push_str(delta);
                self.flush_ordinary().await
            }
            ControlMode::Undecided => {
                self.buffer.push_str(delta);
                self.decide_undecided().await
            }
        }
    }

    /// Finalizes the reply and returns its classification.
    ///
    /// A held ordinary tail is flushed; a directive or late marker stays
    /// suppressed.
    async fn finalize(&mut self) -> ControlPresentation {
        match self.mode {
            ControlMode::Directive => ControlPresentation::Directive,
            ControlMode::LateMarker => ControlPresentation::LateMarker,
            ControlMode::Undecided | ControlMode::Ordinary => {
                if !self.buffer.is_empty() {
                    let text = core::mem::take(&mut self.buffer);
                    let _ = self.push_raw(&text).await;
                }
                ControlPresentation::Ordinary
            }
        }
    }

    /// Presents the owner-derived text after a suppressed directive. The
    /// caller must have scrubbed it already.
    async fn present(&mut self, text: &str) -> DeltaFlow {
        self.push_raw(text).await
    }
}

impl DeltaSink for ControlHoldingSink<'_> {
    fn push_delta<'a>(
        &'a mut self,
        delta: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DeltaFlow> + Send + 'a>> {
        Box::pin(self.push(delta))
    }
}

pub async fn finish_turn(
    turn: Box<DialogueTurn>,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
    scrubber: &impl SecretScrubber,
    task_control: &impl DialogueTaskControlPort,
    sink: &mut (dyn ene_inference::DeltaSink + Send),
    is_current: &(dyn Fn() -> bool + Send + Sync),
) -> DialogueOutcome {
    let DialogueTurn {
        input,
        message,
        prompt,
        authorized,
    } = *turn;
    let (consent_id, consent_rev) = {
        let (id, rev) = authorized.consent_premise();
        (id.to_owned(), rev)
    };
    // Dialogue has no cooperative stop token: it is not a Task Agent
    // execution, so no abort exists to forward. The presentation sink holds
    // back any potential task-control directive line, so provider output can
    // never leak the protocol to the client.
    let mut holder = ControlHoldingSink::new(sink);
    match inference
        .dispatch(authorized, prompt.into_prompt(), &mut holder, None)
        .await
    {
        Ok(InferenceDispatchOutcome::Completed {
            arrival,
            adopted: true,
        }) => {
            let Ok(text) = scrubber.scrub(&arrival.output_text).await else {
                return DialogueOutcome::Interrupted;
            };
            // The provider completed after the last gated delta: confirm
            // the round is still the presented one before anything
            // durable. Generation, consent, credential set, and lifecycle
            // ride the append's atomic compare below; the Host-owned open
            // round lives outside that transaction, so only this check can
            // refuse a reply superseded with no further delta to trip the
            // gate.
            if !is_current() {
                return DialogueOutcome::Interrupted;
            }
            // Finalize the presentation classification before anything is
            // stored: a late marker is a reserved-protocol violation and
            // closes interrupted with no command and no reply row.
            let presentation = holder.finalize().await;
            let (reply_text, reply_credential_set) = match presentation {
                ControlPresentation::LateMarker => return DialogueOutcome::Interrupted,
                ControlPresentation::Ordinary => (text.text().to_owned(), text.credential_set()),
                ControlPresentation::Directive => {
                    let tail = match interpret_task_control(text.text()) {
                        DialogueTaskInterpretation::Command { command } => {
                            match task_control.apply(command, message).await {
                                DialogueTaskControlReply::Answered(tail) => tail,
                                DialogueTaskControlReply::Unavailable => {
                                    return DialogueOutcome::Interrupted;
                                }
                            }
                        }
                        DialogueTaskInterpretation::Invalid => {
                            TASK_CONTROL_CLARIFICATION.to_owned()
                        }
                        // The sink saw a directive the scrubbed text does not
                        // carry; fail closed instead of presenting unproven
                        // text.
                        DialogueTaskInterpretation::Conversation { .. } => {
                            return DialogueOutcome::Interrupted;
                        }
                    };
                    // Scrub before any user-visible publication: the
                    // presented text and the durable reply are the same
                    // scrubbed string.
                    let Ok(scrubbed) = scrubber.scrub(&tail).await else {
                        return DialogueOutcome::Interrupted;
                    };
                    if let DeltaFlow::Abort(_) = holder.present(scrubbed.text()).await {
                        return DialogueOutcome::Interrupted;
                    }
                    (scrubbed.text().to_owned(), scrubbed.credential_set())
                }
            };
            let reply = AppendHistoryCommand {
                companion: input.companion,
                round: input.round,
                role: HistoryRole::Companion,
                text: reply_text.clone(),
                lang: input.lang.clone(),
                at: WallClockWithTz::now(),
                expected_generation: input.generation,
                expected_consent: Some((consent_id, consent_rev)),
                expected_credential_set: Some(reply_credential_set),
                // The durable Owner row this turn committed: the append
                // refuses the reply when a newer accepted Owner input
                // superseded it, even inside the same round.
                expected_owner_message: Some(message),
                local_id: None,
                command_id: None,
                // Same round, same projection; the reply is Host-produced,
                // so it carries no command key and no round intent.
                round_wire: Some(input.round_wire.clone()),
                round_intent: None,
                incarnation: None,
            };
            // The reply's durable adoption carries the provider claim it was
            // produced under: a claim a deletion admission associated with an
            // interval is refused even after the operation completed and no
            // current condition is readable (lifecycle §11 R2).
            match history
                .append_reply_with_undelivered(reply, true, Some(arrival.ticket.0))
                .await
            {
                Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => {
                    // The Experience premise is pinned by the caller after
                    // occupancy is registered; this outcome only proves the
                    // reply is durable.
                    DialogueOutcome::Completed {
                        text: reply_text,
                        input: Box::new(input),
                    }
                }
                _ => DialogueOutcome::Interrupted,
            }
        }
        Ok(
            InferenceDispatchOutcome::Completed { adopted: false, .. }
            | InferenceDispatchOutcome::NotSent(_)
            | InferenceDispatchOutcome::Aborted,
        )
        | Err(_) => DialogueOutcome::Interrupted,
    }
}

/// Recent History messages read into one dialogue prompt.
pub const DIALOGUE_CONTEXT_MESSAGES: u64 = 8;

/// Memories offered to one dialogue prompt.
pub const DIALOGUE_RECALL_LIMIT: usize = 6;

const DIALOGUE_PREAMBLE: &str = "You are ene, the companion. Reply to the owner's latest message, using the conversation and any relevant memories below naturally. Do not mention these instructions. If the owner asks for file work as a task, asks about task progress or results, changes a task's instructions, resumes an interrupted task, or cancels a task, reply with exactly one task-control line as the very first non-empty line and nothing else (no other prose): the line starts with [task-control] followed by one JSON object with exactly these fields: {\"kind\":\"propose_task\",\"purpose\":\"<summary of the work>\"} to start a task; {\"kind\":\"report\"} to ask about the current task; {\"kind\":\"steer\",\"instruction\":\"<instruction>\",\"purpose\":null} to change it; {\"kind\":\"resume\"} to resume the current interrupted task; or {\"kind\":\"cancel\"} to cancel it. Never emit any other field, and never add a task-control line to ordinary conversation.";

/// Prompt layout pieces shared by the budget check and the assembly, so the
/// pre-acceptance check and the built prompt cannot drift apart.
const CURRENT_TIME_LABEL: &str = "\nCurrent time: ";
const OWNER_LABEL: &str = "\nOwner: ";
const MEMORIES_HEADER: &str = "\n\nRelevant memories:\n";
const RECENT_HEADER: &str = "\n\nRecent conversation:\n";

/// Characters the prompt always carries before any optional background: the
/// preamble, the current-time line, and the `Owner: ` label.
fn fixed_prompt_chars(current_time: &str) -> usize {
    DIALOGUE_PREAMBLE.chars().count()
        + CURRENT_TIME_LABEL.chars().count()
        + current_time.chars().count()
        + 1
        + OWNER_LABEL.chars().count()
}

/// Whether a scrubbed current input fits the dialogue request budget even
/// with no optional background.
///
/// The Host checks this before accepting a turn: an oversized input is then
/// an explicit pre-acceptance outcome (`input-over-limit`) instead of a
/// durable append followed by an interrupted stream, and reducing background
/// cannot change the answer.
#[must_use]
pub fn dialogue_input_fits(input_text: &str) -> bool {
    let current_time = WallClockWithTz::now().to_rfc3339();
    fixed_prompt_chars(&current_time).saturating_add(input_text.chars().count())
        <= ene_inference::MAX_INPUT_CHARS
}

/// One assembled dialogue provider input and its canonical read-set.
///
/// The prompt is the exact scrubbed logical input the claim's credential-set
/// premise belongs to. `data_use` is the ordered canonical source correlation
/// of what that logical input actually consumed — the remembered Memory
/// identities (rendered order) and the History message identities
/// (oldest-first rendered order) — so the attempt's provenance names only
/// rows that were really read and the deletion association can hold the
/// claim whose prompt derived from a covered source.
#[derive(Clone, PartialEq, Eq)]
pub struct DialogueInput {
    prompt: ScrubbedText,
    data_use: Vec<RawId>,
}

impl DialogueInput {
    /// Borrows the scrubbed prompt.
    #[must_use]
    pub fn prompt(&self) -> &ScrubbedText {
        &self.prompt
    }

    /// Consumes the input, yielding the scrubbed prompt for dispatch.
    #[must_use]
    pub fn into_prompt(self) -> ScrubbedText {
        self.prompt
    }

    /// The ordered canonical identities the assembled prompt read.
    #[must_use]
    pub fn data_use(&self) -> &[RawId] {
        &self.data_use
    }
}

impl core::fmt::Debug for DialogueInput {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DialogueInput")
            .field("prompt", &"<redacted>")
            .field("data_use_len", &self.data_use.len())
            .finish()
    }
}

/// Builds the dialogue input within the final request budget.
///
/// Priority order: the current input and the fixed labels are secured first;
/// remaining characters go to recent History (newest first) and then to
/// recalled Memory, each selected as whole meaning units. An item that does
/// not fit is skipped rather than truncated, and selection stops at the
/// budget, so the assembled prompt never exceeds
/// [`ene_inference::MAX_INPUT_CHARS`] no matter how large old context grows.
///
/// The current owner input is carried once, after the context sections; it is
/// not yet a durable History identity and is therefore not part of the
/// read-set correlation (the reply append compares it separately as the
/// relied Owner message). The read-set names exactly the Memory and History
/// rows the prompt consumed. Memory content, History text, and the input
/// itself pass through the scrubber before they enter the prompt; a scrub
/// failure is returned so the caller can close the stream without sending or
/// storing raw text. The returned premise is the oldest of every scrubbed
/// piece, so the send claim accepts the prompt only when all pieces were
/// scrubbed under the same current credential set. The memory and history
/// reads stay best-effort: retrieval is derived, so a read failure degrades
/// the context rather than turning a follow-up into an error.
pub async fn assemble_dialogue_input(
    companion: CompanionId,
    input_text: &str,
    history: &impl HistoryRepository,
    learning: &impl LearningRepository,
    scrubber: &impl SecretScrubber,
) -> Result<DialogueInput, SecretScrubError> {
    let recent = history
        .load_recent_timeline(companion, DIALOGUE_CONTEXT_MESSAGES)
        .await
        .unwrap_or_default();
    let recalled = ene_learning::recall(
        learning,
        RecallQuery {
            companion: companion.as_raw(),
            text: input_text.to_owned(),
            limit: DIALOGUE_RECALL_LIMIT,
        },
    )
    .await
    .unwrap_or_default();
    // The input is always scrubbed, and its premise seeds the oldest-premise
    // fold, so the returned set is total without a fallback branch. The
    // scrubbed length is what the budget counts: redaction changes size.
    let input = scrubber.scrub(input_text).await?;
    let mut credential_set = input.credential_set();
    let current_time = WallClockWithTz::now().to_rfc3339();
    let mut budget = ene_inference::MAX_INPUT_CHARS.saturating_sub(
        fixed_prompt_chars(&current_time).saturating_add(input.text().chars().count()),
    );

    // Recent History first, newest to oldest: a fitting older message is
    // still useful when the newest one is too large, and whole messages are
    // never cut. Selection order is reversed for the oldest-first rendering.
    let mut chosen_history: Vec<(RawId, String)> = Vec::new();
    let mut history_header = false;
    for item in recent.iter().rev() {
        let text = scrubber.scrub(&item.text).await?;
        credential_set = credential_set.min(text.credential_set());
        let role = match item.role {
            HistoryRole::Owner => "Owner",
            HistoryRole::Companion => "Companion",
        };
        // The source message's own offset-qualified time stays attached:
        // "tomorrow" in a past message is not re-anchored to now.
        let line = format!("{role} [{}]: {}\n", item.at.to_rfc3339(), text.text());
        let header_cost = if history_header {
            0
        } else {
            RECENT_HEADER.chars().count()
        };
        let line_chars = line.chars().count();
        if line_chars + header_cost > budget {
            continue;
        }
        budget -= line_chars + header_cost;
        history_header = true;
        chosen_history.push((item.id, line));
    }
    chosen_history.reverse();

    // Recalled Memory fills what remains, in recall rank order.
    let mut chosen_memories: Vec<(RawId, String)> = Vec::new();
    let mut memories_header = false;
    for memory in &recalled {
        let content = scrubber.scrub(&memory.content).await?;
        credential_set = credential_set.min(content.credential_set());
        let line = format!("- {}\n", content.text());
        let header_cost = if memories_header {
            0
        } else {
            MEMORIES_HEADER.chars().count()
        };
        let line_chars = line.chars().count();
        if line_chars + header_cost > budget {
            continue;
        }
        budget -= line_chars + header_cost;
        memories_header = true;
        chosen_memories.push((memory.id.as_raw(), line));
    }

    let mut prompt = String::new();
    prompt.push_str(DIALOGUE_PREAMBLE);
    prompt.push_str(CURRENT_TIME_LABEL);
    prompt.push_str(&current_time);
    prompt.push('\n');
    if !chosen_memories.is_empty() {
        prompt.push_str(MEMORIES_HEADER);
        for (_, line) in &chosen_memories {
            prompt.push_str(line);
        }
    }
    if !chosen_history.is_empty() {
        prompt.push_str(RECENT_HEADER);
        for (_, line) in &chosen_history {
            prompt.push_str(line);
        }
    }
    prompt.push_str(OWNER_LABEL);
    prompt.push_str(input.text());
    // The read-set follows the logical input: the Memory section renders
    // before the conversation section, so its identities lead; each section
    // keeps its rendered order.
    let mut data_use: Vec<RawId> = chosen_memories.into_iter().map(|(id, _)| id).collect();
    data_use.extend(chosen_history.into_iter().map(|(id, _)| id));
    // Scrub formatting and fragment joins too, without refreshing away the
    // oldest preparation premise if credentials changed during assembly.
    let prompt = scrubber
        .scrub(&prompt)
        .await?
        .with_oldest_premise(credential_set);
    Ok(DialogueInput { prompt, data_use })
}

/// Recent History messages read into one Experience source.
pub const EXPERIENCE_SOURCE_MESSAGES: u64 = 12;

/// Pins the Experience premise of one just-completed turn.
///
/// Reads the bounded recent window once, at reply completion, so the queued
/// pass judges exactly this transcript. Returns [`None`] when the window is
/// empty or unreadable; the caller then skips the pass instead of later
/// re-reading a different window as if it were the same Experience.
///
/// The caller occupies any process-local remainder this candidate belongs
/// to *before* calling this function, so the pinned body cannot exist
/// outside that remainder.
pub async fn pin_experience(
    input: &AcceptedDialogueInput,
    history: &impl HistoryRepository,
) -> Option<ExperienceCandidate> {
    let items = history
        .load_recent_timeline(input.companion, EXPERIENCE_SOURCE_MESSAGES)
        .await
        .ok()?;
    let (first, last) = (items.first()?, items.last()?);
    Some(ExperienceCandidate {
        companion: input.companion.as_raw(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: first.id,
            end: last.id,
        },
        // The ordered per-message provenance the formation claim carries; the
        // coarse range above stays the Summary's evidence reference.
        sources: items.iter().map(|item| item.id).collect(),
        transcript: items
            .iter()
            .map(|item| ExperienceTurn {
                role: match item.role {
                    HistoryRole::Owner => ExperienceRole::Owner,
                    HistoryRole::Companion => ExperienceRole::Companion,
                },
                text: item.text.clone(),
                // The stored History row is the source of truth for when the
                // turn was said; the offset travels with it.
                at: Some(item.at),
            })
            .collect(),
        at: WallClockWithTz::now(),
    })
}

/// Proposes one pinned Experience and lets Learning judge it.
///
/// The caller runs this after a reply is durable; the result never changes
/// whether that reply completed. The transcript is transient source material
/// for this formation only, and the Summary records only a source-range
/// reference back to the retained History. The caller passes the premise
/// pinned at reply completion, never a fresh History window.
pub async fn propose_experience(
    candidate: ExperienceCandidate,
    learning: &impl LearningRepository,
    inference: &impl InferenceExecutor,
    scrubber: &impl SecretScrubber,
) -> Result<FormationDecision, LearningTechnicalError> {
    let adapter = LearningInferenceAdapter { inference };
    ene_learning::form_experience(learning, &adapter, scrubber, candidate).await
}

/// Maps the inference owner boundary onto the opaque Learning inference port.
///
/// The learning consumer and purpose are admitted by `ene-inference`; this
/// adapter never presents the call as dialogue. The prompt's credential-set
/// premise travels into the send claim unchanged.
struct LearningInferenceAdapter<'a, I> {
    inference: &'a I,
}

impl<I: InferenceExecutor + Send + Sync> LearningInference for LearningInferenceAdapter<'_, I> {
    async fn infer(
        &self,
        premise: ene_learning::LearningInferencePremise,
        prompt: ScrubbedText,
    ) -> Result<ene_learning::LearningInferenceAnswer, LearningInferenceError> {
        match self
            .inference
            .admit_learning(premise.data_use().to_vec())
            .await
        {
            Ok(Admission::Admitted(authorized)) => {
                match self
                    .inference
                    .dispatch(*authorized, prompt, &mut DiscardSink, None)
                    .await
                {
                    Ok(InferenceDispatchOutcome::Completed {
                        arrival,
                        adopted: true,
                    }) => Ok(ene_learning::LearningInferenceAnswer {
                        // The durable claim handle: the inference ticket the
                        // formation's commit carries for the erasure-hold
                        // check.
                        claim: ene_learning::LearningClaimRef::from_raw(arrival.ticket.0),
                        answer: arrival.output_text,
                    }),
                    Ok(
                        InferenceDispatchOutcome::Completed { adopted: false, .. }
                        | InferenceDispatchOutcome::NotSent(_)
                        | InferenceDispatchOutcome::Aborted,
                    ) => Err(LearningInferenceError::Declined),
                    Err(error) => Err(LearningInferenceError::Unavailable {
                        reason: error.to_string(),
                    }),
                }
            }
            Ok(Admission::Declined(_)) => Err(LearningInferenceError::Declined),
            Err(error) => Err(LearningInferenceError::Unavailable {
                reason: error.to_string(),
            }),
        }
    }
}

/// One steering proposal from the Owner conversation (H-A caller side).
///
/// A caller builds this from accepted conversation evidence and proposes it:
/// the command carries no adoption identity. `premise` is the caller's
/// relied-on boundary token (the current revision and the purpose identity in
/// force there), `new_purpose` is the purpose text proposed for adoption
/// ([`None`] keeps the current purpose), and `instruction_source` references
/// the utterance record proposing the additional instruction — provenance,
/// never the adoption identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeSteeringCommand {
    pub premise: SteeringPremiseRef,
    /// Proposed purpose body; [`None`] means the purpose is unchanged.
    pub new_purpose: Option<TaskPurpose>,
    /// Reference to the utterance record proposing the additional
    /// instruction. The Task owner mints a fresh adoption identity; this
    /// reference is provenance only.
    pub instruction_source: RawId,
}

/// Proposes one steering change to the Task owner and returns its decision.
///
/// The caller only proposes. This maps the command onto the Task owner's
/// value premise and delegates to [`ene_task::orchestrate_steering`], which
/// compares the relied-on revision and purpose, mints the new revision's
/// context entry identities, and commits. The returned
/// [`TaskProposalOutcome`] is the owner's outcome unchanged (accepted, stale,
/// missing, or exhausted), never reinterpreted here. A caller never mints
/// entry identities and never names `expected.revision + 1`.
pub async fn propose_steering(
    command: ProposeSteeringCommand,
    repository: &impl TaskRepository,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    ene_task::orchestrate_steering(
        repository,
        SteeringProposalPremise {
            premise: command.premise,
            new_purpose: command.new_purpose,
            instruction_source: command.instruction_source,
        },
    )
    .await
}

/// Proposes one conversation-sourced steering change, conditional on the
/// relied Owner input still being current.
///
/// Identical to [`propose_steering`] except that the Task owner compares the
/// Owner-message currentness premise inside the revision commit: a newer
/// accepted Owner input supersedes the turn and answers
/// [`TaskProposalOutcome::Superseded`] with zero writes.
pub async fn propose_steering_current(
    command: ProposeSteeringCommand,
    repository: &impl ConversationTaskRepository,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    orchestrate_steering_current(
        repository,
        SteeringProposalPremise {
            premise: command.premise,
            new_purpose: command.new_purpose,
            instruction_source: command.instruction_source,
        },
        currentness,
    )
    .await
}

/// One Task proposal from the Owner conversation (H-A caller side).
///
/// The dialogue layer builds this from accepted conversation evidence — the
/// Owner's request is a canonical History record and `origin` references it;
/// the purpose text is the proposal, never a committed Task state. The command
/// carries no identity: `orchestrate_task_creation` mints the Task, context
/// entry, and confirmed workspace association identities. A workspace need is
/// present only when the conversation resolved one; the Task owner confirms
/// the association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeTaskCommand {
    /// The Companion proposing the Task; adopted as the Task assignee.
    pub requester: CompanionId,
    /// The purpose text proposed for adoption.
    pub purpose: TaskPurpose,
    /// The conversation record the request came from.
    pub origin: TaskContextOrigin,
    /// The workspace conditions the request relies on, when any.
    pub workspace_need: Option<WorkspaceNeedRef>,
}

/// Proposes one Task to the Task owner and returns its decision.
///
/// The caller only proposes. This maps the command onto the Task owner's
/// value premise and delegates to [`ene_task::orchestrate_task_creation`],
/// which mints every identity and commits the creation unit. The returned
/// [`TaskProposalOutcome`] is the owner's outcome unchanged
/// ([`TaskProposalOutcome::AcceptedAsTask`] on success). Creating a delegation
/// is a separate owner request issued by the composition root that received
/// the accepted reference; this function never mints a
/// [`DelegationId`](ene_task::DelegationId) and never starts an execution.
pub async fn propose_task(
    command: ProposeTaskCommand,
    repository: &impl TaskRepository,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
    orchestrate_task_creation(
        repository,
        TaskProposalPremise {
            requester: AssigneeRef {
                companion: command.requester.as_raw(),
            },
            purpose: command.purpose,
            origin: command.origin,
            workspace_need: command.workspace_need,
        },
    )
    .await
}

/// Proposes one conversation-sourced Task, conditional on the relied Owner
/// input still being current.
///
/// Identical to [`propose_task`] except that the Task owner compares the
/// Owner-message currentness premise inside the creation transaction: a newer
/// accepted Owner input supersedes the turn and answers
/// [`TaskCreationOutcome::Superseded`] with zero writes.
pub async fn propose_task_current(
    command: ProposeTaskCommand,
    repository: &impl ConversationTaskRepository,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskCreationOutcome, TaskTechnicalError> {
    orchestrate_task_creation_current(
        repository,
        TaskProposalPremise {
            requester: AssigneeRef {
                companion: command.requester.as_raw(),
            },
            purpose: command.purpose,
            origin: command.origin,
            workspace_need: command.workspace_need,
        },
        currentness,
    )
    .await
}

/// Marker introducing one companion-emitted Task control directive.
///
/// The companion's reply may carry at most one final line beginning with this
/// marker; the line is stripped before the reply is stored or shown.
pub const TASK_CONTROL_MARKER: &str = "[task-control]";

/// One Task control command the companion emitted in a dialogue reply.
///
/// The companion owns the interpretation of its own provider output into this
/// closed-world command; the composition root maps it onto the existing Task
/// owner operations. The command deliberately carries no Task identity: the
/// target is the conversation's current Task, resolved by the composition
/// root, so a model output can never name an arbitrary Task. The
/// `instruction` / `purpose` bodies are redacted from [`core::fmt::Debug`].
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DialogueTaskCommand {
    /// Propose a new Task. The Workspace is deliberately absent: it is the
    /// trusted first-party premise, never provider output.
    ProposeTask { purpose: String },
    /// Ask for the current Task's progress / completion report.
    Report,
    /// Propose an additional instruction for the current Task.
    Steer {
        instruction: String,
        purpose: Option<String>,
    },
    /// Request cancel of the current Task.
    Cancel,
    /// Request an explicit resume of the current Task. The command carries
    /// no fields: the Host composes the Task, the premise, and the Owner
    /// instruction reference from its own durable state, so a model output
    /// can never name a Task, revision, purpose, or body. A missing or
    /// ambiguous target is answered with a clarification, never executed.
    Resume,
}

impl core::fmt::Debug for DialogueTaskCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ProposeTask { purpose } => formatter
                .debug_struct("ProposeTask")
                .field("purpose", &"[redacted]")
                .field("purpose_len", &purpose.chars().count())
                .finish(),
            Self::Report => formatter.write_str("Report"),
            Self::Steer {
                instruction,
                purpose,
            } => formatter
                .debug_struct("Steer")
                .field("instruction", &"[redacted]")
                .field("instruction_len", &instruction.chars().count())
                .field("purpose", &purpose.as_ref().map(|_| "[redacted]"))
                .finish(),
            Self::Cancel => formatter.write_str("Cancel"),
            Self::Resume => formatter.write_str("Resume"),
        }
    }
}

/// Interpretation of one companion reply for Task control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueTaskInterpretation {
    /// No control marker: ordinary conversation text, unchanged.
    Conversation { text: String },
    /// The reply is exactly one valid control command line. Provider prose is
    /// deliberately not part of a control reply.
    Command { command: DialogueTaskCommand },
    /// The reply begins with the marker but is not exactly one well-formed
    /// command line; nothing may be executed and the caller answers a
    /// clarification.
    Invalid,
}

/// Interprets one companion reply for an embedded Task control command.
///
/// `[task-control]` is a reserved internal protocol: it may appear only as the
/// reply's first non-empty line, it must be the only non-empty line, and its
/// JSON must be one closed-world command. A reply with no marker at all is
/// ordinary conversation; a marker anywhere else — after prose, mid-line, with
/// trailing prose, malformed, or repeated — is invalid and fails closed: the
/// caller executes no Task operation and stores no reply text containing the
/// protocol. A control reply therefore has no provider prose, so only the
/// scrubbed typed owner outcome is ever presented or stored.
#[must_use]
pub fn interpret_task_control(text: &str) -> DialogueTaskInterpretation {
    if !text.contains(TASK_CONTROL_MARKER) {
        return DialogueTaskInterpretation::Conversation {
            text: text.to_owned(),
        };
    }
    let lines: Vec<&str> = text.lines().collect();
    let Some(first_non_empty) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return DialogueTaskInterpretation::Invalid;
    };
    let trimmed = lines[first_non_empty].trim_start();
    if !trimmed.starts_with(TASK_CONTROL_MARKER) {
        return DialogueTaskInterpretation::Invalid;
    }
    let trailing_prose = lines
        .iter()
        .skip(first_non_empty + 1)
        .any(|line| !line.trim().is_empty());
    let body = trimmed
        .strip_prefix(TASK_CONTROL_MARKER)
        .unwrap_or("")
        .trim();
    match (trailing_prose, parse_task_command(body)) {
        (false, Some(command)) => DialogueTaskInterpretation::Command { command },
        _ => DialogueTaskInterpretation::Invalid,
    }
}

/// Parses the JSON body of one control directive, closed world.
///
/// Each command shape has an exact field set: unknown fields, missing
/// required fields, and wrong types are all invalid, so a provider cannot
/// smuggle an extension (for example a Workspace path) into the command.
fn parse_task_command(body: &str) -> Option<DialogueTaskCommand> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let fields = value.as_object()?.len();
    let command: DialogueTaskCommand = serde_json::from_value(value).ok()?;
    match &command {
        // Serde accepts extra fields on internally tagged unit variants.
        DialogueTaskCommand::Report | DialogueTaskCommand::Cancel | DialogueTaskCommand::Resume
            if fields != 1 =>
        {
            None
        }
        DialogueTaskCommand::ProposeTask { purpose } if purpose.trim().is_empty() => None,
        DialogueTaskCommand::Steer { instruction, .. } if instruction.trim().is_empty() => None,
        _ => Some(command),
    }
}

/// The composition root's answer to one companion Task control command.
///
/// `Unavailable` is the technical-failure class: the turn closes interrupted
/// instead of storing a reply no owner operation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueTaskControlReply {
    /// The owner answered; `text` is the user-facing reply composed from the
    /// typed outcome.
    Answered(String),
    /// The operation could not answer; the turn closes interrupted.
    Unavailable,
}

/// Executes one companion-emitted Task control command.
///
/// The companion owns the interpretation and the turn order; the composition
/// root owns the owner operations. Implementations must use the existing Task
/// owner boundaries (proposal, steering, cancel, report reads) and never
/// write Task state directly. `origin` is the committed Owner message
/// identity of the turn, used as the proposal / instruction provenance.
#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; the composition root implements it"
)]
pub trait DialogueTaskControlPort: Send + Sync {
    async fn apply(&self, command: DialogueTaskCommand, origin: RawId) -> DialogueTaskControlReply;
}

/// Display certainty of one Action attempt in a Task report.
///
/// A companion-owned projection of the Action owner's closed world, so the
/// report layer never re-decides certainty: the composition root maps the
/// owner's read verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskReportCertainty {
    ConfirmedSuccess,
    ConfirmedFailure,
    Unknown,
}

/// One Action attempt as shown in a Task report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportAttempt {
    /// Closed-world operation label (list/read/create/edit).
    pub operation: String,
    /// The target recorded when the attempt started.
    pub target: String,
    /// The Action owner's certainty, projected verbatim.
    pub certainty: TaskReportCertainty,
}

/// User-facing Task report facts composed from canonical owner reads.
///
/// The composition root fills this from durable facts only — `task.progress`,
/// the adopted/unadopted `task_result` body and its verified correlation, and
/// the `action_attempt` records — and this layer owns the report layout. It
/// carries no durable state and rewrites no owner fact: a report never
/// rewrites certainty, progress, or adoption, and an `Unknown` effect is
/// reported as unknown, never as success or failure.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskReport {
    pub progress: TaskProgress,
    /// The current workspace association folder, when one exists.
    pub workspace_folder: Option<String>,
    /// The association's save target, when declared.
    pub save_target: Option<String>,
    /// The sealed final result body, adopted or not.
    pub result_body: Option<String>,
    /// Whether the result body was adopted as the Task completion.
    pub result_adopted: bool,
    /// The verified result-local correlation, when a result exists.
    pub correlated_attempts: Vec<TaskReportAttempt>,
    /// Every other Action attempt under the Task.
    pub other_attempts: Vec<TaskReportAttempt>,
}

impl core::fmt::Debug for TaskReport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskReport")
            .field("progress", &self.progress)
            .field("workspace_folder", &self.workspace_folder)
            .field("save_target", &self.save_target)
            .field(
                "result_body",
                &self.result_body.as_ref().map(|_| "[redacted]"),
            )
            .field("result_adopted", &self.result_adopted)
            .field("correlated_attempts", &self.correlated_attempts)
            .field("other_attempts", &self.other_attempts)
            .finish()
    }
}

impl TaskReport {
    /// Renders the user-facing report from the canonical facts.
    ///
    /// Confirmed create/edit targets are the completed changes, the workspace
    /// folder is the save location, and attempts that are not
    /// [`TaskReportCertainty::ConfirmedSuccess`] are listed as remaining or
    /// unconfirmed effects instead of being folded into success.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = format!("task status: {}", progress_label(self.progress));
        if let Some(folder) = &self.workspace_folder {
            text.push_str(&format!("\nworkspace: {folder}"));
        }
        if let Some(save_target) = &self.save_target {
            text.push_str(&format!("\nsave target: {save_target}"));
        }
        match (&self.result_body, self.result_adopted) {
            (Some(body), true) => {
                text.push_str(&format!("\nresult (adopted): {}", body.trim()));
            }
            (Some(body), false) => {
                text.push_str(&format!(
                    "\nresult (recorded, not adopted): {}",
                    body.trim()
                ));
            }
            (None, _) => text.push_str("\nresult: none"),
        }
        if !self.correlated_attempts.is_empty() {
            text.push_str("\nresult-correlated attempts:");
            for attempt in &self.correlated_attempts {
                text.push_str(&format!("\n- {}", attempt_label(attempt)));
            }
        }
        let changes: Vec<&TaskReportAttempt> = self
            .correlated_attempts
            .iter()
            .chain(self.other_attempts.iter())
            .filter(|attempt| is_completed_change(attempt))
            .collect();
        text.push_str("\ncompleted changes:");
        if changes.is_empty() {
            text.push_str("\n- none");
        } else {
            for attempt in changes {
                text.push_str(&format!("\n- {} {}", attempt.operation, attempt.target));
            }
        }
        let remaining: Vec<&TaskReportAttempt> = self
            .correlated_attempts
            .iter()
            .chain(self.other_attempts.iter())
            .filter(|attempt| attempt.certainty != TaskReportCertainty::ConfirmedSuccess)
            .collect();
        text.push_str("\nremaining/unconfirmed effects:");
        if remaining.is_empty() {
            text.push_str("\n- none");
        } else {
            for attempt in remaining {
                text.push_str(&format!("\n- {}", attempt_label(attempt)));
            }
        }
        text
    }
}

fn progress_label(progress: TaskProgress) -> &'static str {
    match progress {
        TaskProgress::Started => "started",
        TaskProgress::InProgress => "in-progress",
        TaskProgress::Completed => "completed",
        TaskProgress::Failed => "failed",
        TaskProgress::Cancelled => "cancelled",
    }
}

fn is_completed_change(attempt: &TaskReportAttempt) -> bool {
    attempt.certainty == TaskReportCertainty::ConfirmedSuccess
        && matches!(attempt.operation.as_str(), "create" | "edit")
}

fn attempt_label(attempt: &TaskReportAttempt) -> String {
    let certainty = match attempt.certainty {
        TaskReportCertainty::ConfirmedSuccess => "confirmed success",
        TaskReportCertainty::ConfirmedFailure => "confirmed failure",
        TaskReportCertainty::Unknown => "unknown",
    };
    format!("{} {} ({certainty})", attempt.operation, attempt.target)
}

#[cfg(test)]
mod report_tests {
    use super::{TaskProgress, TaskReport, TaskReportAttempt, TaskReportCertainty};

    fn attempt(operation: &str, target: &str, certainty: TaskReportCertainty) -> TaskReportAttempt {
        TaskReportAttempt {
            operation: operation.to_owned(),
            target: target.to_owned(),
            certainty,
        }
    }

    #[test]
    fn a_completed_report_names_the_changes_the_location_and_no_remainder() {
        let report = TaskReport {
            progress: TaskProgress::Completed,
            workspace_folder: Some(String::from("/srv/workspace/ene")),
            save_target: None,
            result_body: Some(String::from("report.md was created")),
            result_adopted: true,
            correlated_attempts: vec![attempt(
                "create",
                "/srv/workspace/ene/report.md",
                TaskReportCertainty::ConfirmedSuccess,
            )],
            other_attempts: Vec::new(),
        };
        let rendered = report.render();
        assert!(rendered.contains("task status: completed"), "{rendered}");
        assert!(
            rendered.contains("workspace: /srv/workspace/ene"),
            "{rendered}"
        );
        assert!(rendered.contains("report.md was created"), "{rendered}");
        assert!(
            rendered.contains("create /srv/workspace/ene/report.md"),
            "{rendered}"
        );
        assert!(
            rendered.contains("remaining/unconfirmed effects:\n- none"),
            "{rendered}"
        );
    }

    #[test]
    fn an_unknown_effect_is_reported_as_unknown_not_as_success_or_failure() {
        let report = TaskReport {
            progress: TaskProgress::Cancelled,
            workspace_folder: None,
            save_target: None,
            result_body: None,
            result_adopted: false,
            correlated_attempts: Vec::new(),
            other_attempts: vec![
                attempt(
                    "create",
                    "/srv/workspace/ene/half.md",
                    TaskReportCertainty::Unknown,
                ),
                attempt(
                    "edit",
                    "/srv/workspace/ene/notes.md",
                    TaskReportCertainty::ConfirmedFailure,
                ),
            ],
        };
        let rendered = report.render();
        assert!(rendered.contains("task status: cancelled"), "{rendered}");
        assert!(rendered.contains("result: none"), "{rendered}");
        assert!(
            rendered.contains("create /srv/workspace/ene/half.md (unknown)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("edit /srv/workspace/ene/notes.md (confirmed failure)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("completed changes:\n- none"),
            "an unconfirmed effect is never listed as a completed change: {rendered}"
        );
    }

    #[test]
    fn the_debug_rendering_redacts_the_result_body() {
        let report = TaskReport {
            progress: TaskProgress::Completed,
            workspace_folder: None,
            save_target: None,
            result_body: Some(String::from("private final words")),
            result_adopted: true,
            correlated_attempts: Vec::new(),
            other_attempts: Vec::new(),
        };
        let rendered = format!("{report:?}");
        assert!(!rendered.contains("private final words"), "{rendered}");
    }
}

#[cfg(test)]
mod task_control_tests {
    use super::{DialogueTaskCommand, DialogueTaskInterpretation, interpret_task_control};

    fn command(text: &str) -> DialogueTaskCommand {
        match interpret_task_control(text) {
            DialogueTaskInterpretation::Command { command } => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn an_ordinary_reply_is_conversation_unchanged() {
        match interpret_task_control("hello there\nsecond line") {
            DialogueTaskInterpretation::Conversation { text } => {
                assert_eq!(text, "hello there\nsecond line");
            }
            other => panic!("expected conversation, got {other:?}"),
        }
    }

    #[test]
    fn a_marker_after_prose_is_invalid() {
        // The marker is reserved: after prose (or mid-line) it fails closed
        // instead of leaking through as ordinary conversation.
        for text in [
            "Sure, I will do that.\n[task-control] {\"kind\":\"report\"}",
            "hello [task-control] {\"kind\":\"report\"}",
            "hello\n[task-control] {\"kind\":\"cancel\"}\nmore",
        ] {
            assert!(
                matches!(
                    interpret_task_control(text),
                    DialogueTaskInterpretation::Invalid
                ),
                "{text:?} must fail closed"
            );
        }
    }

    #[test]
    fn every_closed_world_command_parses_from_the_first_line() {
        assert_eq!(
            command("[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\"}"),
            DialogueTaskCommand::ProposeTask {
                purpose: String::from("read input.txt"),
            }
        );
        assert_eq!(
            command("[task-control] {\"kind\":\"report\"}"),
            DialogueTaskCommand::Report
        );
        assert_eq!(
            command(
                "[task-control] {\"kind\":\"steer\",\"instruction\":\"add a summary\",\"purpose\":null}"
            ),
            DialogueTaskCommand::Steer {
                instruction: String::from("add a summary"),
                purpose: None,
            }
        );
        assert_eq!(
            command("[task-control] {\"kind\":\"cancel\"}"),
            DialogueTaskCommand::Cancel
        );
        assert_eq!(
            command("[task-control] {\"kind\":\"resume\"}"),
            DialogueTaskCommand::Resume
        );
        // Leading blank lines are allowed; the first non-empty line is the
        // directive.
        assert_eq!(
            command("\n\n[task-control] {\"kind\":\"report\"}\n"),
            DialogueTaskCommand::Report
        );
    }

    #[test]
    fn extra_fields_are_invalid_for_every_command_shape() {
        for text in [
            "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\",\"workspace\":\"/etc\"}",
            "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\",\"save_target\":\"/etc\"}",
            "[task-control] {\"kind\":\"report\",\"extra\":1}",
            "[task-control] {\"kind\":\"steer\",\"instruction\":\"add\",\"purpose\":null,\"workspace\":\"/etc\"}",
            "[task-control] {\"kind\":\"cancel\",\"reason\":\"because\"}",
            "[task-control] {\"kind\":\"resume\",\"task\":\"other\"}",
        ] {
            assert!(
                matches!(
                    interpret_task_control(text),
                    DialogueTaskInterpretation::Invalid
                ),
                "{text:?} must be invalid: a provider cannot smuggle extra fields"
            );
        }
    }

    #[test]
    fn malformed_or_non_first_markers_are_invalid() {
        for text in [
            "[task-control] not json",
            "[task-control] {\"kind\":\"unknown\"}",
            "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"\"}",
            "[task-control] {\"kind\":\"steer\",\"instruction\":\"  \"}",
            "[task-control] {\"kind\":\"report\"}\nmore text after",
            "[task-control] {\"kind\":\"report\"}\n[task-control] {\"kind\":\"cancel\"}",
        ] {
            assert!(
                matches!(
                    interpret_task_control(text),
                    DialogueTaskInterpretation::Invalid
                ),
                "{text:?} must be invalid"
            );
        }
    }

    #[test]
    fn the_command_debug_redacts_instruction_bodies() {
        let rendered = format!(
            "{:?}",
            command("[task-control] {\"kind\":\"propose_task\",\"purpose\":\"private purpose\"}")
        );
        assert!(!rendered.contains("private purpose"), "{rendered}");
        let rendered = format!(
            "{:?}",
            command("[task-control] {\"kind\":\"steer\",\"instruction\":\"private instruction\"}")
        );
        assert!(!rendered.contains("private instruction"), "{rendered}");
    }
}

/// The recalled-memory fixture resolves through the real `recall` ranking
/// path via `recall_candidates`, so the final boundary pass actually covers
/// memory content instead of a fixture that swallowed it.
#[cfg(test)]
mod assembly_tests;
