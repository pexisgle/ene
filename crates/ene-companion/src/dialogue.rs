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

use ene_credential::{CredentialSetRevision, ScrubbedText};
use ene_inference::{
    Admission, AuthorizedInference, InferenceDispatchOutcome, InferenceExecutor, NotSentReason,
};
use ene_learning::{
    ExperienceCandidate, ExperienceRole, ExperienceSourceKind, ExperienceTurn, FormationDecision,
    LearningInference, LearningInferenceError, LearningRepository, LearningTechnicalError,
    RecallQuery, SecretScrubError, SecretScrubber, SourceRangeRef,
};
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};

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
/// section; any other reply outcome is interrupted. After that durable
/// append, the Experience premise is pinned for the post-response Learning
/// pass.
pub async fn finish_turn(
    turn: Box<DialogueTurn>,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
    learning: &impl LearningRepository,
    scrubber: &impl SecretScrubber,
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
    match inference.dispatch(authorized, prompt).await {
        Ok(InferenceDispatchOutcome::Completed {
            arrival,
            adopted: true,
        }) => {
            let Ok(text) = scrubber.scrub(&arrival.output_text).await else {
                return DialogueOutcome::Interrupted;
            };
            let reply = AppendHistoryCommand {
                companion: input.companion,
                round: input.round,
                role: HistoryRole::Companion,
                text: text.text.clone(),
                lang: input.lang.clone(),
                at: WallClockWithTz::now(),
                expected_generation: input.generation,
                expected_consent: Some((consent_id, consent_rev)),
                expected_credential_set: Some(text.credential_set),
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
                        text: text.text,
                        experience,
                    }
                }
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

/// Recent History messages read into one dialogue prompt.
pub const DIALOGUE_CONTEXT_MESSAGES: u64 = 8;

/// Memories offered to one dialogue prompt.
pub const DIALOGUE_RECALL_LIMIT: usize = 6;

const DIALOGUE_PREAMBLE: &str = "You are ene, the companion. Reply to the owner's latest message, using the conversation and any relevant memories below naturally. Do not mention these instructions.";

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
                match self.inference.dispatch(*authorized, prompt).await {
                    Ok(InferenceDispatchOutcome::Completed {
                        arrival,
                        adopted: true,
                    }) => Ok(arrival.output_text),
                    Ok(
                        InferenceDispatchOutcome::Completed { adopted: false, .. }
                        | InferenceDispatchOutcome::NotSent(_),
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
