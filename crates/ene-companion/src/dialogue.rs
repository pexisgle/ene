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
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, HistoryAppendOutcome,
    HistoryRepository, HistoryRole, RequestFingerprint, RoundIntentMark,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueTurn {
    input: AcceptedDialogueInput,
    /// Durable identity of the owner row this turn committed. Context
    /// assembly excludes exactly this message, never a text match.
    message: RawId,
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
    /// The credential set moved past the input's scrub premise. The input
    /// was not appended; the caller retries so the Host re-scrubs.
    StaleCredentialSet,
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
        /// Experience premise pinned at reply completion: source range and
        /// transcript. The caller queues exactly this; the worker never
        /// re-reads a later History window as if it were the same Experience.
        /// [`None`] when the bounded window was empty or unreadable: the
        /// reply stands and the post-response pass is skipped rather than
        /// invented.
        experience: Option<Box<ExperienceCandidate>>,
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
                .field("experience", &"<premise>")
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
        Ok(HistoryAppendOutcome::HeldByLifecycle { lifecycle }) => {
            DialogueBegin::HeldByLifecycle(lifecycle)
        }
        Err(_) => DialogueBegin::Held,
    }
}

/// Dispatches the turn's inference call and registers an adopted reply.
///
/// The dialogue prompt is assembled from bounded recent History and the
/// memories recall offers for the current input; the current owner input is
/// excluded from the recent-context section by its committed message
/// identity, never by comparing text, so an earlier identical message stays
/// in the window. Retrieval is derived and best-effort: a history or recall
/// read failure degrades to less context rather than failing a reply, and a
/// suppressed Memory is simply absent. A secret-boundary failure is not
/// degraded: the owner input and the provider output both pass through the
/// scrubber before they reach a model or durable History, and an unprovable
/// boundary closes the stream interrupted instead of sending or storing raw
/// text. The dispatch carries the prompt's credential-set premise, so the
/// send claim refuses a prompt that predates a credential registration. A
/// never-sent or technical outcome closes the stream interrupted; usage
/// accounting is already decided inside the inference boundary. An adopted
/// reply appends with its undelivered registration in the same atomic
/// section; any other reply outcome is interrupted. Provider deltas are
/// pushed to `sink` as they arrive, each gated on a current presentation
/// premise; a delta shown before an invalidation stays as historical
/// partial presentation, never rewound. `is_current` runs once more after
/// provider completion as an early, best-effort refusal of a superseded
/// reply: it only avoids a doomed append attempt. Durable adoption
/// authority stays inside the append transaction — the reply carries the
/// turn's Owner message identity as its premise, and the store refuses the
/// append when a newer accepted Owner input committed first, even inside
/// the same round. After the durable append, the Experience premise is
/// pinned for the post-response Learning pass. A reply carrying a
/// `[task-control]` directive is interpreted before the append: the
/// composition root's [`DialogueTaskControlPort`] executes the command
/// through the existing owner boundaries, the directive line is stripped from
/// storage, and the stored reply is the owner-derived text. A malformed
/// directive clarifies without executing anything; `Unavailable` closes the
/// Presentation sink that never lets a `[task-control]` directive reach the
/// user-visible stream.
///
/// Ordinary text streams as it arrives: complete lines are published unless
/// they begin the directive, and the trailing partial line is published
/// unless it is still a prefix of the marker (the only text that could grow
/// into a directive). Once a line beginning with [`TASK_CONTROL_MARKER`]
/// completes — or the trailing partial reaches the full marker — that text
/// and everything after it are suppressed, and only the owner-derived reply
/// tail is presented. This keeps live streaming for ordinary chat while
/// keeping the streamed text equal to the durable reply for control turns.
struct ControlHoldingSink<'a> {
    inner: &'a mut (dyn DeltaSink + Send),
    pending: String,
    marker_seen: bool,
}

impl<'a> ControlHoldingSink<'a> {
    fn new(inner: &'a mut (dyn DeltaSink + Send)) -> Self {
        Self {
            inner,
            pending: String::new(),
            marker_seen: false,
        }
    }

    /// Finalizes the trailing partial: a line that begins the marker
    /// withholds itself and switches to marker mode, so an unterminated
    /// directive is still never presented. Returns whether the reply carries
    /// a directive.
    fn finalize_marker(&mut self) -> bool {
        if !self.marker_seen && self.pending.trim_start().starts_with(TASK_CONTROL_MARKER) {
            self.marker_seen = true;
            self.pending.clear();
        }
        self.marker_seen
    }

    async fn push(&mut self, delta: &str) -> DeltaFlow {
        if self.marker_seen {
            return DeltaFlow::Continue;
        }
        self.pending.push_str(delta);
        // Publish every complete line that is not a directive.
        while let Some(newline) = self.pending.find('\n') {
            if self.pending[..newline]
                .trim_start()
                .starts_with(TASK_CONTROL_MARKER)
            {
                self.marker_seen = true;
                self.pending.clear();
                return DeltaFlow::Continue;
            }
            let line: String = self.pending.drain(..=newline).collect();
            if let DeltaFlow::Abort(reason) = self.inner.push_delta(&line).await {
                self.pending.clear();
                return DeltaFlow::Abort(reason);
            }
        }
        // The trailing partial is streamed immediately unless it could still
        // grow into a directive line; only a marker prefix is withheld, so
        // ordinary chat keeps its live streaming behaviour.
        let trimmed = self.pending.trim_start();
        if trimmed.starts_with(TASK_CONTROL_MARKER) {
            self.marker_seen = true;
            self.pending.clear();
            return DeltaFlow::Continue;
        }
        if trimmed.len() < TASK_CONTROL_MARKER.len() && TASK_CONTROL_MARKER.starts_with(trimmed) {
            return DeltaFlow::Continue;
        }
        if !self.pending.is_empty() {
            let pending = core::mem::take(&mut self.pending);
            if let DeltaFlow::Abort(reason) = self.inner.push_delta(&pending).await {
                return DeltaFlow::Abort(reason);
            }
        }
        DeltaFlow::Continue
    }

    /// Flushes an ordinary (non-directive) reply's trailing partial line.
    ///
    /// [`Self::finalize_marker`] must have run first, so a trailing directive
    /// has already switched to marker mode.
    async fn finish_ordinary(&mut self) -> DeltaFlow {
        if self.marker_seen || self.pending.is_empty() {
            return DeltaFlow::Continue;
        }
        let pending = core::mem::take(&mut self.pending);
        self.inner.push_delta(&pending).await
    }

    /// Presents the owner-derived tail after a suppressed directive.
    async fn present(&mut self, text: &str) -> DeltaFlow {
        self.inner.push_delta(text).await
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

/// Joins the pre-directive reply text with the owner-derived tail exactly as
/// the stream presented it.
///
/// [`interpret_task_control`] returns the pre-directive prefix with its line
/// terminator, and the presentation sink already published that prefix, so
/// appending the tail reconstructs the durable reply verbatim.
fn control_reply_text(pre: &str, tail: &str) -> String {
    format!("{pre}{tail}")
}

/// The clarification shown for a malformed task-control directive.
const TASK_CONTROL_CLARIFICATION: &str =
    "I could not interpret the task instruction; nothing was changed.";

#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is one distinct owner boundary the turn composes; grouping them would restate the boundary set"
)]
pub async fn finish_turn(
    turn: Box<DialogueTurn>,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
    learning: &impl LearningRepository,
    scrubber: &impl SecretScrubber,
    task_control: &impl DialogueTaskControlPort,
    sink: &mut (dyn ene_inference::DeltaSink + Send),
    is_current: &(dyn Fn() -> bool + Send + Sync),
) -> DialogueOutcome {
    let DialogueTurn {
        input,
        message,
        authorized,
    } = *turn;
    let (consent_id, consent_rev) = {
        let (id, rev) = authorized.consent_premise();
        (id.to_owned(), rev)
    };
    let Ok(prompt) = assemble_dialogue_input(
        input.companion,
        message,
        &input.text,
        history,
        learning,
        scrubber,
    )
    .await
    else {
        return DialogueOutcome::Interrupted;
    };
    // Dialogue has no cooperative stop token: it is not a Task Agent
    // execution, so no abort exists to forward. The presentation sink holds
    // back any potential task-control directive line, so provider output can
    // never leak the protocol to the client.
    let mut holder = ControlHoldingSink::new(sink);
    match inference
        .dispatch(authorized, prompt, &mut holder, None)
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
            // The companion interprets its own output: a trailing
            // task-control directive is executed through the composition
            // root's port before anything is stored, and the stored reply is
            // the owner-derived text. The directive line itself is never
            // stored or shown, and the streamed text is the same reply the
            // History row carries. A technical failure closes the stream
            // interrupted rather than storing a reply no operation produced.
            // Finalize the trailing partial before deciding: an unterminated
            // directive line suppresses itself and counts as a directive.
            let task_control_present = holder.finalize_marker();
            let (reply_text, reply_credential_set) = if task_control_present {
                match interpret_task_control(&text.text) {
                    DialogueTaskInterpretation::Command { text: pre, command } => {
                        match task_control.apply(command, message).await {
                            DialogueTaskControlReply::Answered(tail) => {
                                if let DeltaFlow::Abort(_) = holder.present(&tail).await {
                                    return DialogueOutcome::Interrupted;
                                }
                                let Ok(scrubbed_tail) = scrubber.scrub(&tail).await else {
                                    return DialogueOutcome::Interrupted;
                                };
                                (
                                    control_reply_text(&pre, &scrubbed_tail.text),
                                    scrubbed_tail.credential_set,
                                )
                            }
                            DialogueTaskControlReply::Unavailable => {
                                return DialogueOutcome::Interrupted;
                            }
                        }
                    }
                    DialogueTaskInterpretation::Invalid { text: pre } => {
                        if let DeltaFlow::Abort(_) =
                            holder.present(TASK_CONTROL_CLARIFICATION).await
                        {
                            return DialogueOutcome::Interrupted;
                        }
                        (
                            control_reply_text(&pre, TASK_CONTROL_CLARIFICATION),
                            text.credential_set,
                        )
                    }
                    // The raw stream showed a directive the scrubbed text no
                    // longer carries; fail closed instead of presenting an
                    // unproven reply.
                    DialogueTaskInterpretation::Conversation { .. } => {
                        return DialogueOutcome::Interrupted;
                    }
                }
            } else {
                // Ordinary reply: flush the held trailing partial line so the
                // presented text is the whole reply. A stale presentation
                // premise is best-effort; durable adoption authority stays in
                // the append transaction.
                let _ = holder.finish_ordinary().await;
                (text.text.clone(), text.credential_set)
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
            match history.append_reply_with_undelivered(reply, true).await {
                Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => {
                    // Pin the Experience premise only after the reply is
                    // durable; the queued pass judges exactly this window.
                    let experience = pin_experience(&input, history).await.map(Box::new);
                    DialogueOutcome::Completed {
                        text: reply_text,
                        experience,
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

const DIALOGUE_PREAMBLE: &str = "You are ene, the companion. Reply to the owner's latest message, using the conversation and any relevant memories below naturally. Do not mention these instructions. If the owner asks for file work as a task, asks about task progress or results, changes a task's instructions, or cancels a task, end your reply with exactly one task-control line and nothing after it. The line starts with [task-control] followed by one JSON object: {\"kind\":\"propose_task\",\"purpose\":\"<summary of the work>\",\"workspace\":\"<folder path or null>\",\"save_target\":null} to start a task; {\"kind\":\"report\"} to ask about the current task; {\"kind\":\"steer\",\"instruction\":\"<instruction>\",\"purpose\":null} to change it; or {\"kind\":\"cancel\"} to cancel it. Never add a task-control line to ordinary conversation.";

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

/// Builds the dialogue input within the final request budget.
///
/// Priority order: the current input and the fixed labels are secured first;
/// remaining characters go to recent History (newest first) and then to
/// recalled Memory, each selected as whole meaning units. An item that does
/// not fit is skipped rather than truncated, and selection stops at the
/// budget, so the assembled prompt never exceeds
/// [`ene_inference::MAX_INPUT_CHARS`] no matter how large old context grows.
///
/// The current owner input is carried once, after the context sections, and
/// is excluded from the recent-context window by `current_message` identity.
/// Memory content, History text, and the input itself pass through the
/// scrubber before they enter the prompt; a scrub failure is returned so the
/// caller can close the stream without sending or storing raw text. The
/// returned premise is the oldest of every scrubbed piece, so the send claim
/// accepts the prompt only when all pieces were scrubbed under the same
/// current credential set. The memory and history reads stay best-effort:
/// retrieval is derived, so a read failure degrades the context rather than
/// turning a follow-up into an error.
async fn assemble_dialogue_input(
    companion: CompanionId,
    current_message: RawId,
    input_text: &str,
    history: &impl HistoryRepository,
    learning: &impl LearningRepository,
    scrubber: &impl SecretScrubber,
) -> Result<ScrubbedText, SecretScrubError> {
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
    let mut credential_set = input.credential_set;
    let current_time = WallClockWithTz::now().to_rfc3339();
    let mut budget = ene_inference::MAX_INPUT_CHARS.saturating_sub(
        fixed_prompt_chars(&current_time).saturating_add(input.text.chars().count()),
    );

    // Recent History first, newest to oldest: a fitting older message is
    // still useful when the newest one is too large, and whole messages are
    // never cut. Selection order is reversed for the oldest-first rendering.
    let mut chosen_history: Vec<String> = Vec::new();
    let mut history_header = false;
    for item in recent
        .iter()
        .filter(|item| item.id != current_message)
        .rev()
    {
        let text = scrubber.scrub(&item.text).await?;
        credential_set = credential_set.min(text.credential_set);
        let role = match item.role {
            HistoryRole::Owner => "Owner",
            HistoryRole::Companion => "Companion",
        };
        // The source message's own offset-qualified time stays attached:
        // "tomorrow" in a past message is not re-anchored to now.
        let line = format!("{role} [{}]: {}\n", item.at.to_rfc3339(), text.text);
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
        chosen_history.push(line);
    }
    chosen_history.reverse();

    // Recalled Memory fills what remains, in recall rank order.
    let mut chosen_memories: Vec<String> = Vec::new();
    let mut memories_header = false;
    for memory in &recalled {
        let content = scrubber.scrub(&memory.content).await?;
        credential_set = credential_set.min(content.credential_set);
        let line = format!("- {}\n", content.text);
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
        chosen_memories.push(line);
    }

    let mut prompt = String::new();
    prompt.push_str(DIALOGUE_PREAMBLE);
    prompt.push_str(CURRENT_TIME_LABEL);
    prompt.push_str(&current_time);
    prompt.push('\n');
    if !chosen_memories.is_empty() {
        prompt.push_str(MEMORIES_HEADER);
        for line in &chosen_memories {
            prompt.push_str(line);
        }
    }
    if !chosen_history.is_empty() {
        prompt.push_str(RECENT_HEADER);
        for line in &chosen_history {
            prompt.push_str(line);
        }
    }
    prompt.push_str(OWNER_LABEL);
    prompt.push_str(&input.text);
    Ok(ScrubbedText {
        text: prompt,
        credential_set,
    })
}

/// Recent History messages read into one Experience source.
pub const EXPERIENCE_SOURCE_MESSAGES: u64 = 12;

/// Pins the Experience premise of one just-completed turn.
///
/// Reads the bounded recent window once, at reply completion, so the queued
/// pass judges exactly this transcript. Returns [`None`] when the window is
/// empty or unreadable; the caller then skips the pass instead of later
/// re-reading a different window as if it were the same Experience.
async fn pin_experience(
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
    async fn infer(&self, prompt: ScrubbedText) -> Result<String, LearningInferenceError> {
        match self.inference.admit_learning().await {
            Ok(Admission::Admitted(authorized)) => {
                match self
                    .inference
                    .dispatch(*authorized, prompt, &mut DiscardSink, None)
                    .await
                {
                    Ok(InferenceDispatchOutcome::Completed {
                        arrival,
                        adopted: true,
                    }) => Ok(arrival.output_text),
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
#[derive(Clone, PartialEq, Eq)]
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
        }
    }
}

/// Interpretation of one companion reply for Task control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueTaskInterpretation {
    /// No control marker: ordinary conversation text, unchanged.
    Conversation { text: String },
    /// Exactly one valid control command; `text` is the reply with the
    /// directive line removed.
    Command {
        text: String,
        command: DialogueTaskCommand,
    },
    /// The marker was present but malformed or repeated; nothing may be
    /// executed and `text` is the reply with the directive line removed.
    Invalid { text: String },
}

/// Interprets one companion reply for an embedded Task control command.
///
/// The protocol is closed-world: exactly one `[task-control] {json}` line, as
/// the final non-empty line of the reply. Zero markers is ordinary
/// conversation, exactly one well-formed final marker is a command, and a
/// malformed, repeated, or non-final marker is invalid: the caller answers a
/// clarification and executes nothing. The returned text never contains a
/// directive line, so no marker is ever stored in History or shown to the
/// Owner.
#[must_use]
pub fn interpret_task_control(text: &str) -> DialogueTaskInterpretation {
    let lines: Vec<&str> = text.lines().collect();
    let marker_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with(TASK_CONTROL_MARKER))
        .map(|(index, _)| index)
        .collect();
    if marker_lines.is_empty() {
        return DialogueTaskInterpretation::Conversation {
            text: text.to_owned(),
        };
    }
    let first_marker = marker_lines[0];
    // The pre-directive prefix is the reply text before the directive line,
    // including the line terminator that separated them: the presentation
    // stream published exactly those complete lines, and the durable reply
    // must match the streamed text.
    let cleaned = if first_marker == 0 {
        String::new()
    } else {
        let mut prefix = lines[..first_marker].join("\n");
        prefix.push('\n');
        prefix
    };
    let last_non_empty = lines.iter().rposition(|line| !line.trim().is_empty());
    let command = if marker_lines.len() == 1 && Some(first_marker) == last_non_empty {
        let body = lines[first_marker]
            .trim_start()
            .strip_prefix(TASK_CONTROL_MARKER)
            .unwrap_or("")
            .trim();
        parse_task_command(body)
    } else {
        None
    };
    match command {
        Some(command) => DialogueTaskInterpretation::Command {
            text: cleaned,
            command,
        },
        None => DialogueTaskInterpretation::Invalid { text: cleaned },
    }
}

/// Parses the JSON body of one control directive, closed world.
fn parse_task_command(body: &str) -> Option<DialogueTaskCommand> {
    use serde_json::Value;

    fn optional_string(
        object: &serde_json::Map<String, Value>,
        key: &str,
    ) -> Option<Option<String>> {
        match object.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) => Some(Some(text.clone())),
            Some(_) => None,
        }
    }

    let value: Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    match object.get("kind")?.as_str()? {
        "propose_task" => {
            let purpose = object.get("purpose")?.as_str()?.to_owned();
            if purpose.trim().is_empty() {
                return None;
            }
            Some(DialogueTaskCommand::ProposeTask { purpose })
        }
        "report" => Some(DialogueTaskCommand::Report),
        "steer" => {
            let instruction = object.get("instruction")?.as_str()?.to_owned();
            if instruction.trim().is_empty() {
                return None;
            }
            Some(DialogueTaskCommand::Steer {
                instruction,
                purpose: optional_string(object, "purpose")?,
            })
        }
        "cancel" => Some(DialogueTaskCommand::Cancel),
        _ => None,
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
            DialogueTaskInterpretation::Command { command, .. } => command,
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
    fn every_closed_world_command_parses_and_strips_the_marker() {
        let propose = command(
            "Sure, I will do that.\n[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\"}",
        );
        assert_eq!(
            propose,
            DialogueTaskCommand::ProposeTask {
                purpose: String::from("read input.txt"),
            }
        );
        // A provider-injected workspace field is ignored: it is not part of
        // the closed world, and the Workspace authority is the trusted
        // first-party premise.
        assert_eq!(
            command(
                "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\",\"workspace\":\"/etc\"}"
            ),
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
        match interpret_task_control("Working on it.\n[task-control] {\"kind\":\"report\"}") {
            DialogueTaskInterpretation::Command { text, .. } => {
                assert_eq!(
                    text, "Working on it.\n",
                    "the pre-directive prefix keeps its line terminator so the durable reply matches the streamed text"
                );
            }
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn malformed_repeated_or_non_final_markers_are_invalid() {
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
                    DialogueTaskInterpretation::Invalid { .. }
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
