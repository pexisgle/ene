use ene_credential::{CredentialSetRevision, ScrubbedText};
use ene_inference::{
    Admission, AuthorizedInference, DeltaFlow, DeltaSink, DiscardSink, InferenceDispatchOutcome,
    InferenceExecutor,
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
    SteeringProposalPremise, TaskContextOrigin, TaskProgress, TaskProposalOutcome,
    TaskProposalPremise, TaskPurpose, TaskTechnicalError, WorkspaceNeedRef,
    orchestrate_steering_current, orchestrate_task_creation_current,
};

use crate::{
    ActionCertaintyWire, AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle,
    CompanionTechnicalError, HistoryAppendOutcome, HistoryMessage, HistoryRepository, HistoryRole,
    RequestFingerprint, RoundIntentMark,
};

#[derive(Clone, PartialEq, Eq)]
pub struct AcceptedDialogueInput {
    pub companion: CompanionId,
    pub round: RawId,
    pub generation: PresenceGeneration,
    pub text: String,
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

#[derive(Clone, PartialEq, Eq)]
pub struct DialogueTurn {
    input: AcceptedDialogueInput,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueBegin {
    Ready(Box<DialogueTurn>),
    Replayed { round_wire: Option<String> },
    StaleExpected { current: PresenceGeneration },
    StaleConsent,
    StaleCredentialSet,
    Conflict,
    Held,
    HeldForErasure,
    HeldByLifecycle(CompanionLifecycle),
}

#[derive(Clone, PartialEq, Eq)]
pub enum DialogueOutcome {
    Completed { input: Box<AcceptedDialogueInput> },
    Interrupted,
}

impl core::fmt::Debug for DialogueOutcome {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Completed { .. } => formatter
                .debug_struct("Completed")
                .field("input", &"<accepted>")
                .finish(),
            Self::Interrupted => formatter.write_str("Interrupted"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayClassification {
    Replay { round_wire: Option<String> },
    Conflict,
    None,
    Held,
}

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
                    round_wire: found.round_wire,
                }
            } else {
                ReplayClassification::Conflict
            }
        }
    }
}

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
        Ok(HistoryAppendOutcome::StaleOwnerInput) => DialogueBegin::Held,
        Ok(HistoryAppendOutcome::CommandConflict) => DialogueBegin::Conflict,
        Ok(HistoryAppendOutcome::HeldForErasure) => DialogueBegin::HeldForErasure,
        Ok(HistoryAppendOutcome::HeldByLifecycle { lifecycle }) => {
            DialogueBegin::HeldByLifecycle(lifecycle)
        }
        Err(_) => DialogueBegin::Held,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlPresentation {
    Ordinary,
    Directive,
    LateMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlMode {
    Undecided,
    Ordinary,
    Directive,
    LateMarker,
}

struct ControlHoldingSink<'a> {
    inner: &'a mut (dyn DeltaSink + Send),
    buffer: String,
    mode: ControlMode,
}

impl<'a> ControlHoldingSink<'a> {
    fn new(inner: &'a mut (dyn DeltaSink + Send)) -> Self {
        Self {
            inner,
            buffer: String::new(),
            mode: ControlMode::Undecided,
        }
    }

    async fn push_raw(&mut self, text: &str) -> DeltaFlow {
        self.inner.push_delta(text).await
    }

    async fn flush_ordinary(&mut self) -> DeltaFlow {
        if let Some(position) = self.buffer.find(TASK_CONTROL_MARKER) {
            let prefix = self.buffer[..position].to_owned();
            self.buffer.clear();
            self.mode = ControlMode::LateMarker;
            if prefix.is_empty() {
                return DeltaFlow::Continue;
            }
            return self.push_raw(&prefix).await;
        }
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

    async fn decide_undecided(&mut self) -> DeltaFlow {
        let Some(start) = self
            .buffer
            .find(|character: char| !character.is_whitespace())
        else {
            return DeltaFlow::Continue;
        };
        if let Some(position) = self.buffer.find(TASK_CONTROL_MARKER) {
            if position == start {
                self.mode = ControlMode::Directive;
                self.buffer.clear();
                return DeltaFlow::Continue;
            }
            return self.flush_ordinary().await;
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
}

impl DeltaSink for ControlHoldingSink<'_> {
    fn push_delta<'a>(
        &'a mut self,
        delta: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DeltaFlow> + Send + 'a>> {
        Box::pin(self.push(delta))
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the turn's repositories, sink, currentness premise, and cooperative stop are each owned by a different boundary; a parameter struct would only restate the same wiring"
)]
pub async fn finish_turn(
    turn: Box<DialogueTurn>,
    history: &impl HistoryRepository,
    inference: &impl InferenceExecutor,
    scrubber: &impl SecretScrubber,
    task_control: &impl DialogueTaskControlPort,
    sink: &mut (dyn ene_inference::DeltaSink + Send),
    is_current: &(dyn Fn() -> bool + Send + Sync),
    abort: Option<&ene_inference::DispatchAbort>,
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
    let mut holder = ControlHoldingSink::new(sink);
    match inference
        .dispatch(authorized, prompt.into_prompt(), &mut holder, abort)
        .await
    {
        Ok(InferenceDispatchOutcome::Completed {
            arrival,
            adopted: true,
        }) => {
            let Ok(text) = scrubber.scrub(&arrival.output_text).await else {
                return DialogueOutcome::Interrupted;
            };
            if !is_current() {
                return DialogueOutcome::Interrupted;
            }
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
                            return DialogueOutcome::Interrupted;
                        }
                        DialogueTaskInterpretation::Conversation => {
                            return DialogueOutcome::Interrupted;
                        }
                    };
                    let Ok(scrubbed) = scrubber.scrub(&tail).await else {
                        return DialogueOutcome::Interrupted;
                    };
                    if let DeltaFlow::Abort(_) = holder.push_raw(scrubbed.text()).await {
                        return DialogueOutcome::Interrupted;
                    }
                    (scrubbed.text().to_owned(), scrubbed.credential_set())
                }
            };
            let reply = AppendHistoryCommand {
                companion: input.companion,
                round: input.round,
                role: HistoryRole::Companion,
                text: reply_text,
                lang: input.lang.clone(),
                at: WallClockWithTz::now(),
                expected_generation: input.generation,
                expected_consent: Some((consent_id, consent_rev)),
                expected_credential_set: Some(reply_credential_set),
                expected_owner_message: Some(message),
                local_id: None,
                command_id: None,
                round_wire: Some(input.round_wire.clone()),
                round_intent: None,
                incarnation: None,
            };
            match history
                .append_reply_with_undelivered(reply, Some(arrival.ticket.0))
                .await
            {
                Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => DialogueOutcome::Completed {
                    input: Box::new(input),
                },
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

pub(crate) const DIALOGUE_CONTEXT_MESSAGES: u64 = 8;

pub(crate) const DIALOGUE_RECALL_LIMIT: usize = 6;

const DIALOGUE_PREAMBLE: &str = "You are ene, the companion. Reply to the owner's latest message, using the conversation and any relevant memories below naturally. Do not mention these instructions. If the owner asks for file work as a task, asks about task progress or results, changes a task's instructions, resumes an interrupted task, or cancels a task, reply with exactly one task-control line as the very first non-empty line and nothing else (no other prose): the line starts with [task-control] followed by one JSON object with exactly these fields: {\"kind\":\"propose_task\",\"purpose\":\"<summary of the work>\"} to start a task; {\"kind\":\"report\"} to ask about the current task; {\"kind\":\"steer\",\"instruction\":\"<instruction>\",\"purpose\":null} to change it; {\"kind\":\"resume\"} to resume the current interrupted task; or {\"kind\":\"cancel\"} to cancel it. Never emit any other field, and never add a task-control line to ordinary conversation.";

const CURRENT_TIME_LABEL: &str = "\nCurrent time: ";
const OWNER_LABEL: &str = "\nOwner: ";
const MEMORIES_HEADER: &str = "\n\nRelevant memories:\n";
const RECENT_HEADER: &str = "\n\nRecent conversation:\n";

fn fixed_prompt_chars(current_time: &str) -> usize {
    DIALOGUE_PREAMBLE.chars().count()
        + CURRENT_TIME_LABEL.chars().count()
        + current_time.chars().count()
        + 1
        + OWNER_LABEL.chars().count()
}

#[must_use]
pub fn dialogue_input_fits(input_text: &str) -> bool {
    let current_time = WallClockWithTz::now().to_rfc3339_secs();
    fixed_prompt_chars(&current_time).saturating_add(input_text.chars().count())
        <= ene_inference::MAX_INPUT_CHARS
}

#[derive(Clone, PartialEq, Eq)]
pub struct DialogueInput {
    prompt: ScrubbedText,
    data_use: Vec<RawId>,
}

impl DialogueInput {
    #[must_use]
    pub fn into_prompt(self) -> ScrubbedText {
        self.prompt
    }

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

fn fit_within_budget(
    lines: Vec<(RawId, String)>,
    header: &str,
    budget: &mut usize,
) -> Vec<(RawId, String)> {
    let mut header_charged = false;
    let mut chosen = Vec::new();
    for (id, line) in lines {
        let header_cost = if header_charged {
            0
        } else {
            header.chars().count()
        };
        let line_chars = line.chars().count();
        if line_chars + header_cost > *budget {
            continue;
        }
        *budget -= line_chars + header_cost;
        header_charged = true;
        chosen.push((id, line));
    }
    chosen
}

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
    let input = scrubber.scrub(input_text).await?;
    let mut credential_set = input.credential_set();
    let current_time = WallClockWithTz::now().to_rfc3339_secs();
    let mut budget = ene_inference::MAX_INPUT_CHARS.saturating_sub(
        fixed_prompt_chars(&current_time).saturating_add(input.text().chars().count()),
    );

    let mut history_lines: Vec<(RawId, String)> = Vec::new();
    for item in recent.iter().rev() {
        let text = scrubber.scrub(&item.text).await?;
        credential_set = credential_set.min(text.credential_set());
        let role = match item.role {
            HistoryRole::Owner => "Owner",
            HistoryRole::Companion => "Companion",
        };
        let line = format!("{role} [{}]: {}\n", item.at.to_rfc3339(), text.text());
        history_lines.push((item.id, line));
    }
    let mut chosen_history = fit_within_budget(history_lines, RECENT_HEADER, &mut budget);
    chosen_history.reverse();

    let mut memory_lines: Vec<(RawId, String)> = Vec::new();
    for memory in &recalled {
        let content = scrubber.scrub(&memory.content).await?;
        credential_set = credential_set.min(content.credential_set());
        memory_lines.push((memory.id.as_raw(), format!("- {}\n", content.text())));
    }
    let chosen_memories = fit_within_budget(memory_lines, MEMORIES_HEADER, &mut budget);

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
    let mut data_use: Vec<RawId> = chosen_memories.into_iter().map(|(id, _)| id).collect();
    data_use.extend(chosen_history.into_iter().map(|(id, _)| id));
    let prompt = scrubber
        .scrub(&prompt)
        .await?
        .with_oldest_premise(credential_set);
    Ok(DialogueInput { prompt, data_use })
}

pub(crate) const EXPERIENCE_SOURCE_MESSAGES: u64 = 12;

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
        sources: items.iter().map(|item| item.id).collect(),
        transcript: items
            .iter()
            .map(|item| ExperienceTurn {
                role: match item.role {
                    HistoryRole::Owner => ExperienceRole::Owner,
                    HistoryRole::Companion => ExperienceRole::Companion,
                },
                text: item.text.clone(),
                at: Some(item.at),
            })
            .collect(),
        at: WallClockWithTz::now(),
    })
}

pub async fn propose_experience(
    candidate: ExperienceCandidate,
    learning: &impl LearningRepository,
    inference: &impl InferenceExecutor,
    scrubber: &impl SecretScrubber,
    abort: Option<&ene_inference::DispatchAbort>,
) -> Result<FormationDecision, LearningTechnicalError> {
    let adapter = LearningInferenceAdapter { inference, abort };
    ene_learning::form_experience(learning, &adapter, scrubber, candidate).await
}

struct LearningInferenceAdapter<'a, I> {
    inference: &'a I,
    abort: Option<&'a ene_inference::DispatchAbort>,
}

impl<I: InferenceExecutor + Send + Sync> LearningInference for LearningInferenceAdapter<'_, I> {
    async fn infer(
        &self,
        premise: ene_learning::LearningInferencePremise,
        prompt: ScrubbedText,
    ) -> Result<ene_learning::LearningInferenceAnswer, LearningInferenceError> {
        match self
            .inference
            .admit_learning(premise.data_use.clone())
            .await
        {
            Ok(Admission::Admitted(authorized)) => {
                match self
                    .inference
                    .dispatch(*authorized, prompt, &mut DiscardSink, self.abort)
                    .await
                {
                    Ok(InferenceDispatchOutcome::Completed {
                        arrival,
                        adopted: true,
                    }) => Ok(ene_learning::LearningInferenceAnswer {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeSteeringCommand {
    pub premise: SteeringPremiseRef,
    pub new_purpose: Option<TaskPurpose>,
    pub instruction_source: RawId,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeTaskCommand {
    pub requester: CompanionId,
    pub purpose: TaskPurpose,
    pub origin: TaskContextOrigin,
    pub workspace_need: Option<WorkspaceNeedRef>,
}

pub async fn propose_task_current(
    command: ProposeTaskCommand,
    repository: &impl ConversationTaskRepository,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskProposalOutcome, TaskTechnicalError> {
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

pub(crate) const TASK_CONTROL_MARKER: &str = "[task-control]";

#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DialogueTaskCommand {
    ProposeTask {
        purpose: String,
    },
    Report,
    Steer {
        instruction: String,
        purpose: Option<String>,
    },
    Cancel,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DialogueTaskInterpretation {
    Conversation,
    Command { command: DialogueTaskCommand },
    Invalid,
}

#[must_use]
pub(crate) fn interpret_task_control(text: &str) -> DialogueTaskInterpretation {
    match text.matches(TASK_CONTROL_MARKER).count() {
        0 => return DialogueTaskInterpretation::Conversation,
        1 => {}
        _ => return DialogueTaskInterpretation::Invalid,
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

fn parse_task_command(body: &str) -> Option<DialogueTaskCommand> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let fields = value.as_object()?.len();
    let command: DialogueTaskCommand = serde_json::from_value(value).ok()?;
    match &command {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueTaskControlReply {
    Answered(String),
    Unavailable,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; the composition root implements it"
)]
pub trait DialogueTaskControlPort: Send + Sync {
    async fn apply(&self, command: DialogueTaskCommand, origin: RawId) -> DialogueTaskControlReply;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportAttempt {
    pub operation: String,
    pub target: String,
    pub certainty: ActionCertaintyWire,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TaskReport {
    pub progress: TaskProgress,
    pub workspace_folder: Option<String>,
    pub save_target: Option<String>,
    pub result_body: Option<String>,
    pub result_adopted: bool,
    pub correlated_attempts: Vec<TaskReportAttempt>,
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
            .filter(|attempt| attempt.certainty != ActionCertaintyWire::ConfirmedSuccess)
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
    attempt.certainty == ActionCertaintyWire::ConfirmedSuccess
        && matches!(attempt.operation.as_str(), "create" | "edit")
}

fn attempt_label(attempt: &TaskReportAttempt) -> String {
    let certainty = match attempt.certainty {
        ActionCertaintyWire::ConfirmedSuccess => "confirmed success",
        ActionCertaintyWire::ConfirmedFailure => "confirmed failure",
        ActionCertaintyWire::Unknown => "unknown",
    };
    format!("{} {} ({certainty})", attempt.operation, attempt.target)
}

#[cfg(test)]
mod report_tests {
    use super::{ActionCertaintyWire, TaskProgress, TaskReport, TaskReportAttempt};

    fn attempt(operation: &str, target: &str, certainty: ActionCertaintyWire) -> TaskReportAttempt {
        TaskReportAttempt {
            operation: operation.to_owned(),
            target: target.to_owned(),
            certainty,
        }
    }

    #[test]
    fn completed_and_unknown_reports_keep_effect_certainty_distinct() {
        let cases = [
            (
                TaskReport {
                    progress: TaskProgress::Completed,
                    workspace_folder: Some(String::from("/srv/workspace/ene")),
                    save_target: None,
                    result_body: Some(String::from("report.md was created")),
                    result_adopted: true,
                    correlated_attempts: vec![attempt(
                        "create",
                        "/srv/workspace/ene/report.md",
                        ActionCertaintyWire::ConfirmedSuccess,
                    )],
                    other_attempts: Vec::new(),
                },
                [
                    "task status: completed",
                    "workspace: /srv/workspace/ene",
                    "report.md was created",
                    "create /srv/workspace/ene/report.md",
                    "remaining/unconfirmed effects:\n- none",
                ],
            ),
            (
                TaskReport {
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
                            ActionCertaintyWire::Unknown,
                        ),
                        attempt(
                            "edit",
                            "/srv/workspace/ene/notes.md",
                            ActionCertaintyWire::ConfirmedFailure,
                        ),
                    ],
                },
                [
                    "task status: cancelled",
                    "result: none",
                    "create /srv/workspace/ene/half.md (unknown)",
                    "edit /srv/workspace/ene/notes.md (confirmed failure)",
                    "completed changes:\n- none",
                ],
            ),
        ];

        for (report, expected) in cases {
            let rendered = report.render();
            for expected in expected {
                assert!(rendered.contains(expected), "{rendered}");
            }
        }
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
    fn task_control_parsing_covers_every_shape() {
        assert!(matches!(
            interpret_task_control("hello there\nsecond line"),
            DialogueTaskInterpretation::Conversation
        ));

        let valid = [
            (
                "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\"}",
                DialogueTaskCommand::ProposeTask {
                    purpose: String::from("read input.txt"),
                },
            ),
            (
                "[task-control] {\"kind\":\"report\"}",
                DialogueTaskCommand::Report,
            ),
            (
                "[task-control] {\"kind\":\"steer\",\"instruction\":\"add a summary\",\"purpose\":null}",
                DialogueTaskCommand::Steer {
                    instruction: String::from("add a summary"),
                    purpose: None,
                },
            ),
            (
                "[task-control] {\"kind\":\"cancel\"}",
                DialogueTaskCommand::Cancel,
            ),
            (
                "[task-control] {\"kind\":\"resume\"}",
                DialogueTaskCommand::Resume,
            ),
            (
                "\n\n[task-control] {\"kind\":\"report\"}\n",
                DialogueTaskCommand::Report,
            ),
        ];
        for (text, expected) in valid {
            assert_eq!(command(text), expected, "{text:?}");
        }

        for text in [
            "Sure, I will do that.\n[task-control] {\"kind\":\"report\"}",
            "hello [task-control] {\"kind\":\"report\"}",
            "hello\n[task-control] {\"kind\":\"cancel\"}\nmore",
            "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\",\"workspace\":\"/etc\"}",
            "[task-control] {\"kind\":\"propose_task\",\"purpose\":\"read input.txt\",\"save_target\":\"/etc\"}",
            "[task-control] {\"kind\":\"report\",\"extra\":1}",
            "[task-control] {\"kind\":\"steer\",\"instruction\":\"add\",\"purpose\":null,\"workspace\":\"/etc\"}",
            "[task-control] {\"kind\":\"cancel\",\"reason\":\"because\"}",
            "[task-control] {\"kind\":\"resume\",\"task\":\"other\"}",
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
                "{text:?}"
            );
        }
    }
}

#[cfg(test)]
mod control_sink_tests {
    use super::{ControlHoldingSink, ControlPresentation};
    use ene_inference::{DeltaFlow, DeltaSink};

    #[derive(Default)]
    struct RecordingSink {
        published: String,
    }

    impl DeltaSink for RecordingSink {
        fn push_delta<'a>(
            &'a mut self,
            delta: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DeltaFlow> + Send + 'a>> {
            Box::pin(async move {
                self.published.push_str(delta);
                DeltaFlow::Continue
            })
        }
    }

    async fn run(deltas: &[&str]) -> (String, ControlPresentation) {
        let mut recorder = RecordingSink::default();
        let mut holder = ControlHoldingSink::new(&mut recorder);
        for delta in deltas {
            assert_eq!(holder.push(delta).await, DeltaFlow::Continue);
        }
        let presentation = holder.finalize().await;
        (recorder.published, presentation)
    }

    #[tokio::test]
    async fn late_marker_stream_layouts_publish_the_same_visible_prefix() {
        let cases: &[&[&str]] = &[
            &["hello\n[task-control] {\"kind\":\"cancel\"}"],
            &["hello\n[task-", "control] {\"kind\":\"cancel\"}"],
        ];

        for &deltas in cases {
            let (published, presentation) = run(deltas).await;
            assert_eq!(published, "hello\n");
            assert_eq!(presentation, ControlPresentation::LateMarker);
        }
    }
}
