use ene_primitive::{RawId, WallClockWithTz};
use serde::Deserialize;
use thiserror::Error;

use ene_credential::{ScrubbedText, SecretScrubError, SecretScrubber};

use crate::identity::{LearningClaimRef, MemoryId, SourceRangeRef, SummaryId};
use crate::memory::{ChangeKind, Importance, Memory, TemporalMeaning};
use crate::repository::{
    LearningRepository, LearningTechnicalError, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryTarget,
};
use crate::scope::LearningScope;
use crate::summary::SummaryRecord;

pub const MAX_FORMATION_CHANGES: usize = 5;

pub const MAX_FORMATION_TURNS: usize = 24;

/// Most current memories one formation reads before selecting candidates.
///
/// The read is a bounded newest-first scan: an environment with more
/// memories than this needs an indexed or embedding selection instead.
pub(crate) const FORMATION_SCAN_LIMIT: u64 = 200;

const RECENT_MEMORY_LIMIT: usize = 12;

const RELEVANT_MEMORY_LIMIT: usize = 8;

const EXISTING_MEMORY_LIMIT: usize = RECENT_MEMORY_LIMIT + RELEVANT_MEMORY_LIMIT;

/// Conservative character budget for one assembled formation prompt.
///
/// Kept below the inference boundary's own input cap so the preamble, schema,
/// and the final whole-prompt scrub still fit. Whole memories and turns are
/// dropped rather than cut when including one would exceed the budget; the
/// newest turn is retained regardless, so an oversized experience is refused
/// by the provider instead of silently leaving the prompt without the
/// experience being judged.
const FORMATION_PROMPT_CHAR_BUDGET: usize = 6_000;

const PROMPT_PREAMBLE: &str = "\
You are ene's learning formation pass for one companion.
Decide what, if anything, from the new experience below should be kept or changed as long-term Memory.
Rules:
- Answer with one JSON object and nothing else.
- Write a short summary of what happened and what it means for later understanding.
- Keep only information useful for understanding the owner, the companion, or their shared situations.
- Do not keep small talk, transient states, or raw conversation.
- Weigh an explicit request to remember (for example \"remember this\" or \"覚えておいて\") highly.
- Never keep credential values or secrets, even when asked to remember them.
- When the experience confirms or changes an existing memory, update that memory by its number instead of creating a duplicate.
- A correction is \"corrected_initially_wrong\" when the earlier memory was never true, and \"changed_since\" when it was true until the situation changed.
- Memory scope is always this companion; never claim shared or global scope.";

const PROMPT_SCHEMA: &str = "\
Answer exactly as:
{\"summary\": \"compressed evidence\", \"memories\": [{\"action\": \"create\", \"content\": \"...\", \"importance\": 3, \"temporal\": \"enduring\"}]}
Actions:
- create: a new memory. Content and temporal are required; importance defaults to 3.
- update: change an existing memory by \"target\" number, with \"change\" one of reinforced, refined, integrated, corrected_initially_wrong, changed_since. Target and change are required. Content is the new full content when it changes; omit it to keep the current content. Omitted importance and temporal keep their current values.
- forget: suppress recall of an existing memory by \"target\" number without deleting it. Target is required.
Use an empty \"memories\" list when nothing is worth keeping. Importance is 1-5. Temporal is \"enduring\" for facts and preferences, \"event\" for something that happened.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceRole {
    Owner,
    Companion,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExperienceTurn {
    pub role: ExperienceRole,
    pub text: String,
    pub at: Option<WallClockWithTz>,
}

impl core::fmt::Debug for ExperienceTurn {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ExperienceTurn")
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .field("at", &self.at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceCandidate {
    pub companion: RawId,
    pub source: SourceRangeRef,
    pub sources: Vec<RawId>,
    pub transcript: Vec<ExperienceTurn>,
    pub at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormationDecision {
    Formed { summary: SummaryId },
    NoChangesApplied,
    DeclinedAsNoEndValue,
    DeferredForContext,
    /// The erasure gate refused a change: its proposed content or its
    /// canonical provenance belongs to a deletion interval (lifecycle
    /// §7/§11 R2). That change stored nothing, and the durable association
    /// outlives the operation, so this origin is never retried or re-claimed
    /// under a fresh identity. It stays held until erasure clears; a
    /// genuinely new origin forms from fresh History afterwards. Takes
    /// precedence over [`Self::Formed`] when another change of the same pass
    /// committed before the condition became current.
    HeldForErasure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningInferencePremise {
    pub data_use: Vec<RawId>,
}

/// One inference answer together with the durable claim it ran under.
///
/// The claim is the opaque identity of the provider attempt (the inference
/// ticket). The formation carries it into every commit so the store can
/// refuse a delayed formation whose provenance was associated with a deletion
/// operation, even after that operation completed.
#[derive(Clone, PartialEq, Eq)]
pub struct LearningInferenceAnswer {
    /// Provider output text; redacted from `core::fmt::Debug` because it may
    /// quote owner speech or secret-bearing material before scrubbing.
    pub answer: String,
    pub claim: LearningClaimRef,
}

impl core::fmt::Debug for LearningInferenceAnswer {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LearningInferenceAnswer")
            .field("answer", &"[redacted]")
            .field("claim", &self.claim)
            .finish()
    }
}

/// The model boundary used to judge one Experience.
///
/// Kept as a port so this crate does not depend on inference or permission
/// crates: the Host supplies an implementation through the inference boundary
/// with its own consumer and purpose. The premise carries the formation's
/// canonical source correlation, and the prompt carries the credential-set
/// premise it was scrubbed under, so the send claim can refuse a prompt that
/// predates a credential registration or derives from covered data.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait LearningInference: Send + Sync {
    async fn infer(
        &self,
        premise: LearningInferencePremise,
        prompt: ScrubbedText,
    ) -> Result<LearningInferenceAnswer, LearningInferenceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LearningInferenceError {
    #[error("learning inference was declined")]
    Declined,
    #[error("learning inference unavailable: {reason}")]
    Unavailable { reason: String },
}

pub async fn form_experience(
    repository: &impl LearningRepository,
    inference: &impl LearningInference,
    scrubber: &impl SecretScrubber,
    candidate: ExperienceCandidate,
) -> Result<FormationDecision, LearningTechnicalError> {
    let scope = LearningScope::companion(candidate.companion);
    let scanned = repository
        .list_current_memories(candidate.companion, None, FORMATION_SCAN_LIMIT)
        .await?;
    let existing = select_existing(scanned, &candidate.transcript);
    let (prompt, rendered_memories) = build_prompt(&existing, &candidate, scrubber).await?;
    // The claim's provenance is exactly what the prompt read: the transcript
    // messages pinned at reply completion and the current Memory identities
    // actually rendered into the prompt, in prompt order. It rides the
    // provider claim, so a condition that committed first holds the send, and
    // a deletion admission can associate this formation with its interval.
    let mut data_use = candidate.sources.clone();
    data_use.extend(
        existing[..rendered_memories]
            .iter()
            .map(|memory| memory.id.as_raw()),
    );
    let inferred = match inference
        .infer(LearningInferencePremise { data_use }, prompt)
        .await
    {
        Ok(answer) => answer,
        Err(LearningInferenceError::Declined) => {
            return Ok(FormationDecision::DeferredForContext);
        }
        Err(LearningInferenceError::Unavailable { reason }) => {
            return Err(LearningTechnicalError::InferenceUnavailable { reason });
        }
    };
    let claim = inferred.claim;
    let answer = inferred.answer;
    let Some(answer) = parse_answer(&answer) else {
        return Ok(FormationDecision::DeferredForContext);
    };
    let Some(summary_text) = answer.summary else {
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_text = scrubber
        .scrub(&summary_text)
        .await
        .map_err(secret_boundary_failure)?;
    if answer.memories.is_empty() {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    }
    if summary_text.text().trim().is_empty() {
        // The answer proposed memories but carried no usable summary, so it is
        // ungrounded rather than a judgement of no value.
        return Ok(FormationDecision::DeferredForContext);
    }
    // Resolve every entry before the first commit. One entry the schema
    // cannot interpret makes the whole answer undecidable: committing only
    // the other entries would reconstruct the model's meaning from a partly
    // unreadable answer.
    let Some(proposals) = resolve_model_memories(answer.memories, &existing[..rendered_memories])
    else {
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_id = SummaryId::generate();
    let summary = SummaryRecord {
        id: summary_id,
        scope,
        content: summary_text.text().trim().to_owned(),
        source: candidate.source,
        formed_at: candidate.at,
    };

    let mut prepared = Vec::new();
    let mut applied = false;
    let mut held = false;
    for proposal in proposals {
        let content = scrubber
            .scrub(&proposal.content)
            .await
            .map_err(secret_boundary_failure)?;
        if content.text().trim().is_empty() {
            return Ok(FormationDecision::DeferredForContext);
        }
        prepared.push((
            proposal.target,
            proposal.change,
            content,
            proposal.importance,
            proposal.temporal,
        ));
    }
    let secret_premise = ScrubbedText::oldest_premise(
        std::iter::once(&summary_text).chain(prepared.iter().map(|(_, _, content, _, _)| content)),
    );

    for (target, change, content, importance, temporal) in prepared {
        let content = content.text().trim().to_owned();
        let outcome = repository
            .commit_memory_change(MemoryChangeCommit {
                summary: Some(summary.clone()),
                secret_premise,
                claim: Some(claim),
                change: MemoryChange {
                    target,
                    scope,
                    content,
                    importance,
                    temporal,
                    change,
                    recall_suppressed: change.suppresses_recall(),
                    at: candidate.at,
                },
            })
            .await?;
        match outcome {
            MemoryChangeOutcome::Committed { .. } => applied = true,
            MemoryChangeOutcome::StaleCredentialSet => {
                return Err(LearningTechnicalError::SecretBoundaryUnavailable {
                    reason: String::from("credential set moved during formation"),
                });
            }
            // The erasure gate refused this change; the pass is reported as
            // held, never as a benign no-change, and the durable association
            // survives so the same origin is not retried.
            MemoryChangeOutcome::HeldForErasure => held = true,
            // Stale / missing / scope / duplicate / exhausted rejections are
            // benign: nothing applied and nothing to retry.
            MemoryChangeOutcome::StaleTarget { .. }
            | MemoryChangeOutcome::MissingTarget { .. }
            | MemoryChangeOutcome::ScopeMismatch { .. }
            | MemoryChangeOutcome::AlreadyExists { .. }
            | MemoryChangeOutcome::RevisionExhausted { .. } => {}
        }
    }
    if held {
        return Ok(FormationDecision::HeldForErasure);
    }
    if applied {
        return Ok(FormationDecision::Formed {
            summary: summary_id,
        });
    }
    Ok(FormationDecision::NoChangesApplied)
}

async fn build_prompt(
    existing: &[Memory],
    candidate: &ExperienceCandidate,
    scrubber: &impl SecretScrubber,
) -> Result<(ScrubbedText, usize), LearningTechnicalError> {
    let mut prompt = String::from(PROMPT_PREAMBLE);
    let mut premises = Vec::new();
    let mut rendered_memories = 0_usize;
    prompt.push_str("\n\nExisting memories:\n");
    if existing.is_empty() {
        prompt.push_str("(none)\n");
    } else {
        for (position, memory) in existing.iter().enumerate() {
            let content = scrubber
                .scrub(&memory.content)
                .await
                .map_err(secret_boundary_failure)?;
            let mut line = format!(
                "{}. [importance {}] ",
                position + 1,
                memory.importance.as_u8()
            );
            line.push_str(content.text());
            line.push('\n');
            // Never split a Memory body across the budget; the tail (older,
            // lower-relevance memories) is dropped whole.
            if prompt.chars().count() + line.chars().count() + PROMPT_SCHEMA.chars().count()
                > FORMATION_PROMPT_CHAR_BUDGET
            {
                break;
            }
            premises.push(content.credential_set());
            prompt.push_str(&line);
            rendered_memories += 1;
        }
    }
    prompt.push_str("\nNew experience:\n");
    let window = recent_turns(&candidate.transcript);
    // Fit whole turns from the newest backwards, then render the retained
    // turns oldest-first. The newest turn is retained even when it alone
    // cannot fit: dropping it would let Memory form from a prompt that omits
    // the experience being judged, so the provider's over-limit refusal is
    // the safe outcome instead.
    let mut retained: Vec<(ScrubbedText, String)> = Vec::new();
    let mut retained_chars = 0_usize;
    for turn in window.iter().rev() {
        let text = scrubber
            .scrub(&turn.text)
            .await
            .map_err(secret_boundary_failure)?;
        let mut rendered = String::from(match turn.role {
            ExperienceRole::Owner => "Owner",
            ExperienceRole::Companion => "Companion",
        });
        if let Some(at) = turn.at {
            rendered.push_str(&format!(" [{}]", at.to_rfc3339()));
        }
        rendered.push_str(": ");
        rendered.push_str(text.text());
        rendered.push('\n');
        let fits = prompt.chars().count()
            + retained_chars
            + rendered.chars().count()
            + PROMPT_SCHEMA.chars().count()
            <= FORMATION_PROMPT_CHAR_BUDGET;
        if !fits && !retained.is_empty() {
            break;
        }
        retained_chars += rendered.chars().count();
        retained.push((text, rendered));
    }
    for (text, rendered) in retained.into_iter().rev() {
        premises.push(text.credential_set());
        prompt.push_str(&rendered);
    }
    prompt.push('\n');
    prompt.push_str(PROMPT_SCHEMA);
    prompt.push_str(&format!(
        "\nReturn at most {MAX_FORMATION_CHANGES} memory entries; keep only the most important when more changes seem needed."
    ));
    let scrubbed = scrubber
        .scrub(&prompt)
        .await
        .map_err(secret_boundary_failure)?;
    Ok(match premises.into_iter().min() {
        Some(prior) => (scrubbed.with_oldest_premise(prior), rendered_memories),
        None => (scrubbed, rendered_memories),
    })
}

fn secret_boundary_failure(error: SecretScrubError) -> LearningTechnicalError {
    LearningTechnicalError::SecretBoundaryUnavailable {
        reason: error.to_string(),
    }
}

/// The newest [`MAX_FORMATION_TURNS`] turns of a transcript.
///
/// The prompt window and relevance selection must read the same window, so
/// both callers share this one computation of the start.
fn recent_turns(transcript: &[ExperienceTurn]) -> &[ExperienceTurn] {
    let first = transcript.len().saturating_sub(MAX_FORMATION_TURNS);
    &transcript[first..]
}

/// Selects the existing memories one formation may target.
///
/// The newest [`RECENT_MEMORY_LIMIT`] are always included; the remaining
/// slots go to the older memories with the highest lexical overlap with the
/// new experience. `scanned` arrives newest first and is already bounded by
/// [`FORMATION_SCAN_LIMIT`], so an irrelevant older Memory cannot flood the
/// prompt. The overlap is a selection heuristic, never a stored Memory field
/// and never the model's semantic importance.
fn select_existing(scanned: Vec<Memory>, transcript: &[ExperienceTurn]) -> Vec<Memory> {
    if scanned.len() <= EXISTING_MEMORY_LIMIT {
        return scanned;
    }
    let mut selected: Vec<Memory> = scanned[..RECENT_MEMORY_LIMIT].to_vec();
    let experience = recent_turns(transcript)
        .iter()
        .map(|turn| turn.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let terms = crate::relevance::recall_index_terms(&experience);
    let mut older: Vec<(usize, &Memory)> = scanned[RECENT_MEMORY_LIMIT..]
        .iter()
        .map(|memory| (crate::relevance::overlap(&terms, &memory.content), memory))
        .collect();
    older.sort_by(|(left, _), (right, _)| right.cmp(left));
    selected.extend(
        older
            .into_iter()
            .take(RELEVANT_MEMORY_LIMIT)
            .map(|(_, memory)| memory.clone()),
    );
    selected
}

struct ResolvedChange {
    target: MemoryTarget,
    change: ChangeKind,
    content: String,
    importance: Importance,
    temporal: TemporalMeaning,
}

fn resolve_model_memories(
    entries: Vec<ModelMemory>,
    existing: &[Memory],
) -> Option<Vec<ResolvedChange>> {
    if entries.len() > MAX_FORMATION_CHANGES {
        return None;
    }
    entries
        .into_iter()
        .map(|entry| resolve_model_memory(entry, existing))
        .collect()
}

fn resolve_model_memory(entry: ModelMemory, existing: &[Memory]) -> Option<ResolvedChange> {
    let action = ModelAction::parse(entry.action.as_deref())?;
    let temporal = match entry.temporal.as_deref() {
        Some(value) => Some(parse_temporal(value)?),
        None => None,
    };
    match action {
        ModelAction::Create => {
            let content = entry.content?;
            if content.trim().is_empty() {
                return None;
            }
            Some(ResolvedChange {
                target: MemoryTarget::New {
                    id: MemoryId::generate(),
                },
                change: ChangeKind::Initial,
                content,
                importance: Importance::clamped(
                    entry
                        .importance
                        .unwrap_or_else(|| Importance::default().as_u8()),
                ),
                temporal: temporal?,
            })
        }
        ModelAction::Update => {
            let known = targeted(entry.target, existing)?;
            let change = parse_change(entry.change.as_deref()?)?;
            Some(ResolvedChange {
                target: MemoryTarget::Existing {
                    id: known.id,
                    expected_revision: known.revision,
                },
                change,
                content: match entry.content {
                    Some(content) if content.trim().is_empty() => return None,
                    Some(content) => content,
                    None => known.content.clone(),
                },
                importance: Importance::clamped(
                    entry.importance.unwrap_or_else(|| known.importance.as_u8()),
                ),
                temporal: temporal.unwrap_or(known.temporal),
            })
        }
        ModelAction::Forget => {
            let known = targeted(entry.target, existing)?;
            Some(ResolvedChange {
                target: MemoryTarget::Existing {
                    id: known.id,
                    expected_revision: known.revision,
                },
                change: ChangeKind::Forgotten,
                content: known.content.clone(),
                importance: known.importance,
                temporal: known.temporal,
            })
        }
    }
}

fn targeted(target: Option<usize>, existing: &[Memory]) -> Option<&Memory> {
    target
        .and_then(|index| index.checked_sub(1))
        .and_then(|position| existing.get(position))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelAction {
    Create,
    Update,
    Forget,
}

impl ModelAction {
    fn parse(value: Option<&str>) -> Option<Self> {
        match value? {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "forget" => Some(Self::Forget),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct ModelAnswer {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    memories: Vec<ModelMemory>,
}

#[derive(Deserialize)]
struct ModelMemory {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    target: Option<usize>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    importance: Option<u8>,
    #[serde(default)]
    temporal: Option<String>,
    #[serde(default)]
    change: Option<String>,
}

fn parse_answer(answer: &str) -> Option<ModelAnswer> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&answer[start..=end]).ok()
}

fn parse_temporal(value: &str) -> Option<TemporalMeaning> {
    match value {
        "enduring" => Some(TemporalMeaning::Enduring),
        "event" => Some(TemporalMeaning::Event),
        _ => None,
    }
}

fn parse_change(value: &str) -> Option<ChangeKind> {
    match value {
        "reinforced" => Some(ChangeKind::Reinforced),
        "refined" => Some(ChangeKind::Refined),
        "integrated" => Some(ChangeKind::Integrated),
        "corrected_initially_wrong" => Some(ChangeKind::CorrectedInitiallyWrong),
        "changed_since" => Some(ChangeKind::ChangedSince),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, WallClockWithTz};

    use crate::formation::{
        ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationDecision,
        LearningInferenceError, form_experience,
    };
    use crate::identity::{ExperienceSourceKind, MemoryRevision, SourceRangeRef};
    use crate::repository::{LearningRepository, LearningTechnicalError};
    use crate::scope::LearningScope;
    use crate::test_support::{
        FailingScrubber, FakeLearningRepository, ReplacingScrubber, ScriptedInference,
    };

    fn candidate(companion: RawId, turns: &[(&str, &str)]) -> ExperienceCandidate {
        ExperienceCandidate {
            companion,
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: turns
                .iter()
                .map(|(role, text)| ExperienceTurn {
                    role: match *role {
                        "owner" => ExperienceRole::Owner,
                        _ => ExperienceRole::Companion,
                    },
                    text: (*text).to_owned(),
                    at: None,
                })
                .collect(),
            at: WallClockWithTz::now(),
        }
    }

    fn answer() -> String {
        String::from(
            r#"{"summary": "The owner likes jasmine tea.", "memories": [{"action": "create", "content": "The owner likes jasmine tea.", "importance": 4, "temporal": "enduring"}]}"#,
        )
    }

    #[tokio::test]
    async fn forms_summary_evidence_and_new_memories() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(answer())]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(companion, &[("owner", "remember that I like jasmine tea")]),
        )
        .await
        .unwrap();
        let FormationDecision::Formed { summary } = decision else {
            panic!("a useful experience must form");
        };
        let memories = repository.current();
        let [stored] = memories.as_slice() else {
            panic!("exactly the new memory must be stored: {memories:?}");
        };
        let revisions = repository
            .list_memory_revisions(stored.id, None, 100)
            .await
            .unwrap();
        assert_eq!(stored.revision, MemoryRevision::initial());
        assert_eq!(revisions[0].content, "The owner likes jasmine tea.");
        assert_eq!(revisions[0].scope, LearningScope::companion(companion));
        assert_eq!(revisions[0].importance.as_u8(), 4);
        let evidence = repository
            .load_summaries(&[summary])
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(evidence.content, "The owner likes jasmine tea.");
        assert_eq!(revisions[0].summary, Some(summary));
        assert_eq!(revisions[0].change, crate::memory::ChangeKind::Initial);
    }

    #[tokio::test]
    async fn declines_an_experience_with_no_keepable_information() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "They exchanged greetings.", "memories": []}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(RawId::new(), &[("owner", "hi"), ("companion", "hello")]),
        )
        .await
        .unwrap();
        assert_eq!(decision, FormationDecision::DeclinedAsNoEndValue);
        assert!(
            repository.current().is_empty(),
            "nothing is stored for a declined experience"
        );
    }

    #[tokio::test]
    async fn an_erasure_held_pass_is_reported_as_held() {
        let repository = FakeLearningRepository::new();
        repository.hold_for_erasure();
        let inference = ScriptedInference::new(vec![Ok(answer())]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(
                RawId::new(),
                &[("owner", "remember that I like jasmine tea")],
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            decision,
            FormationDecision::HeldForErasure,
            "a change refused by the erasure gate is held, not a benign no-change"
        );
        assert!(
            repository.current().is_empty(),
            "a held pass stores no Memory"
        );
        assert!(
            repository.summaries().is_empty(),
            "a held pass stores no Summary evidence"
        );
    }

    #[tokio::test]
    async fn malformed_model_answers_store_nothing() {
        let repository = FakeLearningRepository::new();
        let inference =
            ScriptedInference::new(vec![Ok(String::from("I could not decide anything."))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(RawId::new(), &[("owner", "something")]),
        )
        .await
        .unwrap();
        assert_eq!(decision, FormationDecision::DeferredForContext);
        assert!(repository.current().is_empty());
    }

    #[tokio::test]
    async fn a_memory_without_summary_evidence_is_not_stored() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"memories": [{"content": "ungrounded", "importance": 3}]}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(RawId::new(), &[("owner", "something")]),
        )
        .await
        .unwrap();
        assert_eq!(decision, FormationDecision::DeferredForContext);
        assert!(repository.current().is_empty());
    }

    #[tokio::test]
    async fn secrets_never_reach_the_prompt_or_stored_content() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "The owner shared a key: sk-secret.", "memories": [{"action": "create", "content": "The key is sk-secret.", "importance": 5, "temporal": "enduring"}]}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(RawId::new(), &[("owner", "remember my key sk-secret")]),
        )
        .await
        .unwrap();
        assert!(
            matches!(decision, FormationDecision::Formed { .. }),
            "the formation still completes with redacted text"
        );
        let prompts = inference.prompts();
        assert!(
            !prompts[0].contains("sk-secret"),
            "the prompt must not carry the credential: {}",
            prompts[0]
        );
        assert!(prompts[0].contains("[credential]"));
        for memory in repository.current() {
            assert!(
                !memory.content.contains("sk-secret"),
                "stored memory must be redacted: {}",
                memory.content
            );
        }
    }

    #[tokio::test]
    async fn formation_prompt_keeps_each_turns_source_time() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "A deadline was mentioned.", "memories": []}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let candidate = ExperienceCandidate {
            companion: RawId::new(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: vec![
                ExperienceTurn {
                    role: ExperienceRole::Owner,
                    text: String::from("明日提出する"),
                    at: Some(
                        WallClockWithTz::parse_rfc3339("2026-09-12T10:00:00+09:00")
                            .expect("fixture timestamp"),
                    ),
                },
                ExperienceTurn {
                    role: ExperienceRole::Companion,
                    text: String::from("了解した"),
                    at: Some(
                        WallClockWithTz::parse_rfc3339("2026-09-12T00:30:00-05:00")
                            .expect("fixture timestamp"),
                    ),
                },
                ExperienceTurn {
                    role: ExperienceRole::Owner,
                    text: String::from("追記: 変更なし"),
                    at: None,
                },
            ],
            at: WallClockWithTz::now(),
        };
        let _ = form_experience(&repository, &inference, &scrubber, candidate)
            .await
            .unwrap();
        let prompt = &inference.prompts()[0];
        assert!(
            prompt.contains("Owner [2026-09-12T10:00:00+09:00]: 明日提出する"),
            "each turn keeps its own offset-qualified source time: {prompt}"
        );
        assert!(
            prompt.contains("Companion [2026-09-12T00:30:00-05:00]: 了解した"),
            "a different offset stays visible on its own turn: {prompt}"
        );
        assert!(
            prompt.contains("Owner: 追記: 変更なし"),
            "a turn without a recorded time renders without a guessed one: {prompt}"
        );
    }

    #[tokio::test]
    async fn a_scrub_failure_stores_nothing_and_never_prompts() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(answer())]);
        let outcome = form_experience(
            &repository,
            &inference,
            &FailingScrubber,
            candidate(RawId::new(), &[("owner", "something secret")]),
        )
        .await;
        assert!(
            matches!(
                outcome,
                Err(LearningTechnicalError::SecretBoundaryUnavailable { .. })
            ),
            "an unprovable secret boundary must fail closed, got {outcome:?}"
        );
        assert!(
            inference.prompts().is_empty(),
            "raw text must not reach the model when absence cannot be proven"
        );
        assert!(
            repository.current().is_empty(),
            "nothing may be stored when the scrub failed"
        );
    }

    #[tokio::test]
    async fn inference_failure_is_a_technical_error_not_a_formation() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Err(LearningInferenceError::Unavailable {
            reason: String::from("provider down"),
        })]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let outcome = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(RawId::new(), &[("owner", "something")]),
        )
        .await;
        assert!(matches!(
            outcome,
            Err(LearningTechnicalError::InferenceUnavailable { .. })
        ));
        assert!(repository.current().is_empty());
    }

    #[tokio::test]
    async fn the_prompt_shows_existing_memories_and_remember_guidance() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let seeded = crate::repository::MemoryChangeCommit {
            summary: None,
            secret_premise: None,
            claim: None,
            change: crate::repository::MemoryChange {
                target: crate::repository::MemoryTarget::New {
                    id: crate::identity::MemoryId::generate(),
                },
                scope: LearningScope::companion(companion),
                content: String::from("The owner prefers morning conversations."),
                importance: crate::memory::Importance::default(),
                temporal: crate::memory::TemporalMeaning::Enduring,
                change: crate::memory::ChangeKind::Initial,
                recall_suppressed: false,
                at: WallClockWithTz::now(),
            },
        };
        repository.commit_memory_change(seeded).await.unwrap();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "nothing new", "memories": []}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let _ = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(companion, &[("owner", "hello again")]),
        )
        .await
        .unwrap();
        let prompt = &inference.prompts()[0];
        assert!(
            prompt.contains("The owner prefers morning conversations."),
            "existing memories are shown so duplicates can be declined"
        );
        assert!(
            prompt.contains("覚えておいて"),
            "explicit remember requests are weighed by instruction"
        );
    }

    #[tokio::test]
    async fn existing_memories_are_scrubbed_before_the_prompt() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        // Models a durable row written before the value became registered:
        // the prompt must redact it even though the Memory owner already
        // holds it.
        let seeded = crate::repository::MemoryChangeCommit {
            summary: None,
            secret_premise: None,
            claim: None,
            change: crate::repository::MemoryChange {
                target: crate::repository::MemoryTarget::New {
                    id: crate::identity::MemoryId::generate(),
                },
                scope: LearningScope::companion(companion),
                content: String::from("The owner's old key is sk-secret."),
                importance: crate::memory::Importance::default(),
                temporal: crate::memory::TemporalMeaning::Enduring,
                change: crate::memory::ChangeKind::Initial,
                recall_suppressed: false,
                at: WallClockWithTz::now(),
            },
        };
        repository.commit_memory_change(seeded).await.unwrap();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "nothing new", "memories": []}"#,
        ))]);
        let scrubber = ReplacingScrubber::new("sk-secret", "[credential]");
        let _ = form_experience(
            &repository,
            &inference,
            &scrubber,
            candidate(companion, &[("owner", "hello again")]),
        )
        .await
        .unwrap();
        let prompt = &inference.prompts()[0];
        assert!(
            !prompt.contains("sk-secret"),
            "an existing Memory must not carry a registered value into the prompt: {prompt}"
        );
        assert!(prompt.contains("[credential]"));
    }
}

#[cfg(test)]
mod consolidation_tests {
    use ene_primitive::{RawId, WallClockWithTz};

    use crate::formation::{
        ExperienceCandidate, ExperienceRole, ExperienceTurn, FORMATION_PROMPT_CHAR_BUDGET,
        FormationDecision, MAX_FORMATION_CHANGES, form_experience,
    };
    use crate::identity::{ExperienceSourceKind, MemoryRevision, SourceRangeRef};
    use crate::memory::ChangeKind;
    use crate::repository::LearningRepository;
    use crate::test_support::{
        FakeLearningRepository, RacingInference, ReplacingScrubber, ScriptedInference, seed_memory,
    };

    fn candidate(companion: RawId) -> ExperienceCandidate {
        candidate_text(companion, "a follow-up exchange")
    }

    fn candidate_text(companion: RawId, text: &str) -> ExperienceCandidate {
        ExperienceCandidate {
            companion,
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript: vec![ExperienceTurn {
                role: ExperienceRole::Owner,
                text: text.to_owned(),
                at: None,
            }],
            at: WallClockWithTz::now(),
        }
    }

    fn scrubber() -> ReplacingScrubber {
        ReplacingScrubber::new("sk-secret", "[credential]")
    }

    async fn apply_update(
        answer: &str,
    ) -> (
        FakeLearningRepository,
        crate::identity::MemoryId,
        FormationDecision,
    ) {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let (memory, _) = seed_memory(&repository, companion, "owner likes tea").await;
        let inference = ScriptedInference::new(vec![Ok(answer.to_owned())]);
        let decision = form_experience(&repository, &inference, &scrubber(), candidate(companion))
            .await
            .unwrap();
        (repository, memory, decision)
    }

    #[tokio::test]
    async fn repeated_information_reinforces_instead_of_duplicating() {
        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The owner mentioned tea again.", "memories": [{"action": "update", "target": 1, "change": "reinforced", "content": "owner likes tea"}]}"#,
        )
        .await;
        assert!(
            matches!(decision, FormationDecision::Formed { .. }),
            "the reinforcement must apply, got {decision:?}"
        );
        assert_eq!(
            repository.current().len(),
            1,
            "no duplicate memory is created"
        );
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].content, "owner likes tea");
        assert_eq!(revisions[1].change, ChangeKind::Reinforced);
        assert_eq!(
            revisions.last().unwrap().revision,
            MemoryRevision::from_u64(2)
        );
    }

    #[tokio::test]
    async fn refinement_updates_content_and_keeps_the_earlier_revision() {
        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The owner was more specific.", "memories": [{"action": "update", "target": 1, "change": "refined", "content": "owner prefers jasmine tea in the morning"}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(
            revisions[1].content,
            "owner prefers jasmine tea in the morning"
        );
        assert_eq!(
            revisions[0].content, "owner likes tea",
            "the earlier recognition is not rewritten"
        );
        assert_eq!(revisions[1].change, ChangeKind::Refined);
    }

    #[tokio::test]
    async fn initial_wrong_and_changed_since_stay_distinct() {
        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The owner corrected me.", "memories": [{"action": "update", "target": 1, "change": "corrected_initially_wrong", "content": "owner never liked tea"}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions[1].change, ChangeKind::CorrectedInitiallyWrong);

        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The situation changed.", "memories": [{"action": "update", "target": 1, "change": "changed_since", "content": "owner switched to coffee"}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions[1].change, ChangeKind::ChangedSince);
        assert_ne!(
            ChangeKind::CorrectedInitiallyWrong,
            ChangeKind::ChangedSince
        );
    }

    #[tokio::test]
    async fn forgetting_suppresses_recall_without_deleting_content_or_revisions() {
        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The owner asked me to let the topic rest.", "memories": [{"action": "forget", "target": 1}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].content, "owner likes tea");
        assert_eq!(revisions[1].change, ChangeKind::Forgotten);
        assert!(
            revisions[1].recall_suppressed,
            "the suppression is part of the revision history"
        );
    }

    #[tokio::test]
    async fn update_without_importance_keeps_the_existing_one() {
        let (repository, memory, decision) = apply_update(
            r#"{"summary": "A small refinement.", "memories": [{"action": "update", "target": 1, "change": "refined", "content": "owner really likes tea"}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(
            revisions.last().unwrap().importance.as_u8(),
            crate::Importance::default().as_u8(),
            "an omitted importance does not reset the stored one"
        );
    }

    #[tokio::test]
    async fn a_stale_target_is_rejected_not_overwritten() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let (memory, revision) = seed_memory(&repository, companion, "owner likes tea").await;
        let inference = RacingInference::new(
            &repository,
            companion,
            memory,
            revision,
            r#"{"summary": "The owner mentioned tea.", "memories": [{"action": "update", "target": 1, "change": "refined", "content": "owner likes jasmine tea"}]}"#,
        );
        let decision = form_experience(&repository, &inference, &scrubber(), candidate(companion))
            .await
            .unwrap();
        assert!(
            matches!(decision, FormationDecision::NoChangesApplied),
            "a moved target without an erasure hold is a benign no-change, not a hold: {decision:?}"
        );
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(
            revisions.last().unwrap().content,
            "advanced by another formation",
            "the newer recognition is untouched"
        );
        assert_eq!(revisions.len(), 2, "the stale change leaves no revision");
    }

    /// Asserts one undecidable answer leaves current Memory, revisions, and
    /// Summary evidence untouched, whether or not a target was seeded.
    async fn deferred_answer_stores_nothing(seeded: bool, answer: &str) {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let target = if seeded {
            let (memory, revision) = seed_memory(&repository, companion, "owner likes tea").await;
            Some((memory, revision))
        } else {
            None
        };
        let before = repository.current();
        let inference = ScriptedInference::new(vec![Ok(answer.to_owned())]);
        let decision = form_experience(&repository, &inference, &scrubber(), candidate(companion))
            .await
            .unwrap();
        assert_eq!(
            decision,
            FormationDecision::DeferredForContext,
            "an undecidable answer must defer: {answer}"
        );
        assert_eq!(
            repository.current(),
            before,
            "no current Memory may change: {answer}"
        );
        assert!(
            repository.summaries().is_empty(),
            "no Summary evidence may be stored: {answer}"
        );
        if let Some((memory, revision)) = target {
            let revisions = repository
                .list_memory_revisions(memory, None, 100)
                .await
                .unwrap();
            assert_eq!(revisions.len(), 1, "no partial revision may remain");
            assert_eq!(
                revisions.last().unwrap().revision,
                revision,
                "no revision may advance"
            );
        }
    }

    #[tokio::test]
    async fn missing_or_unknown_action_defers_the_whole_answer() {
        for answer in [
            r#"{"summary": "s", "memories": [{"content": "x", "temporal": "enduring"}]}"#,
            r#"{"summary": "s", "memories": [{"action": "merge", "content": "x", "temporal": "enduring"}]}"#,
        ] {
            deferred_answer_stores_nothing(false, answer).await;
        }
    }

    #[tokio::test]
    async fn update_with_missing_or_unknown_change_defers_the_whole_answer() {
        for answer in [
            r#"{"summary": "s", "memories": [{"action": "update", "target": 1, "content": "y"}]}"#,
            r#"{"summary": "s", "memories": [{"action": "update", "target": 1, "change": "merged", "content": "y"}]}"#,
        ] {
            deferred_answer_stores_nothing(true, answer).await;
        }
    }

    #[tokio::test]
    async fn supplied_unknown_temporal_defers_the_whole_answer() {
        deferred_answer_stores_nothing(
            false,
            r#"{"summary": "s", "memories": [{"action": "create", "content": "x", "temporal": "eternal"}]}"#,
        )
        .await;
    }

    #[tokio::test]
    async fn missing_temporal_on_a_new_memory_defers_the_whole_answer() {
        deferred_answer_stores_nothing(
            false,
            r#"{"summary": "s", "memories": [{"action": "create", "content": "x", "importance": 4}]}"#,
        )
        .await;
    }

    #[tokio::test]
    async fn invalid_target_defers_the_whole_answer() {
        for answer in [
            r#"{"summary": "s", "memories": [{"action": "update", "target": 9, "change": "refined", "content": "y"}]}"#,
            r#"{"summary": "s", "memories": [{"action": "update", "change": "refined", "content": "y"}]}"#,
            r#"{"summary": "s", "memories": [{"action": "forget", "target": 0}]}"#,
        ] {
            deferred_answer_stores_nothing(true, answer).await;
        }
    }

    #[tokio::test]
    async fn mixed_valid_and_invalid_entries_store_nothing() {
        deferred_answer_stores_nothing(
            true,
            r#"{"summary": "s", "memories": [{"action": "update", "target": 1, "change": "refined", "content": "y"}, {"action": "explode", "content": "z", "temporal": "enduring"}]}"#,
        )
        .await;
        deferred_answer_stores_nothing(
            true,
            r#"{"summary": "s", "memories": [{"action": "create", "content": "new", "temporal": "enduring"}, {"action": "update", "target": 1, "content": "no change kind"}]}"#,
        )
        .await;
    }

    fn create_entries(count: usize) -> String {
        (0..count)
            .map(|index| {
                format!(
                    r#"{{"action": "create", "content": "memory {index}", "importance": 3, "temporal": "enduring"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[tokio::test]
    async fn an_answer_at_the_change_cap_is_accepted() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let entries = create_entries(MAX_FORMATION_CHANGES);
        let inference = ScriptedInference::new(vec![Ok(format!(
            r#"{{"summary": "s", "memories": [{entries}]}}"#
        ))]);
        let decision = form_experience(&repository, &inference, &scrubber(), candidate(companion))
            .await
            .unwrap();
        assert!(
            matches!(decision, FormationDecision::Formed { .. }),
            "an answer at the cap is still decidable, got {decision:?}"
        );
        assert_eq!(repository.current().len(), MAX_FORMATION_CHANGES);
        assert!(
            inference.prompts()[0].contains(&format!("at most {MAX_FORMATION_CHANGES}")),
            "the prompt states the same cap the parser enforces"
        );
    }

    #[tokio::test]
    async fn an_answer_over_the_change_cap_defers_the_whole_answer() {
        let entries = create_entries(MAX_FORMATION_CHANGES + 1);
        deferred_answer_stores_nothing(
            false,
            &format!(r#"{{"summary": "s", "memories": [{entries}]}}"#),
        )
        .await;
    }

    #[tokio::test]
    async fn a_valid_prefix_is_not_committed_when_the_answer_exceeds_the_cap() {
        let mut entries = vec![
            String::from(
                r#"{"action": "update", "target": 1, "change": "refined", "content": "y"}"#,
            );
            MAX_FORMATION_CHANGES
        ];
        entries.push(String::from(
            r#"{"action": "explode", "content": "z", "temporal": "enduring"}"#,
        ));
        deferred_answer_stores_nothing(
            true,
            &format!(
                r#"{{"summary": "s", "memories": [{}]}}"#,
                entries.join(", ")
            ),
        )
        .await;
    }

    #[tokio::test]
    async fn an_older_relevant_memory_is_reachable_outside_the_newest_window() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        // The oldest memory is the correction target; twenty later ones push
        // it out of any newest-only window of twenty.
        let (memory, _) =
            seed_memory(&repository, companion, "The owner's dog is named Pochi").await;
        for index in 0..20 {
            let _ = seed_memory(
                &repository,
                companion,
                &format!("unrelated note {index} about the weather"),
            )
            .await;
        }
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "The owner corrected the dog's name.", "memories": [{"action": "update", "target": 13, "change": "corrected_initially_wrong", "content": "The owner's dog is named Momo"}]}"#,
        ))]);
        let decision = form_experience(
            &repository,
            &inference,
            &scrubber(),
            candidate_text(companion, "Actually the dog is named Momo, not Pochi."),
        )
        .await
        .unwrap();
        assert!(
            matches!(decision, FormationDecision::Formed { .. }),
            "the older relevant memory must be revisable, got {decision:?}"
        );
        assert_eq!(
            repository.current().len(),
            21,
            "correcting the old memory must not create a duplicate"
        );
        let revisions = repository
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap();
        assert_eq!(revisions[1].content, "The owner's dog is named Momo");
        assert_eq!(revisions[1].revision, MemoryRevision::from_u64(2));
        assert_eq!(
            revisions[0].content, "The owner's dog is named Pochi",
            "the earlier recognition stays"
        );
        assert_eq!(revisions[1].change, ChangeKind::CorrectedInitiallyWrong);
        assert!(
            revisions[1].summary.is_some(),
            "the correction keeps its grounds"
        );
        let prompt = &inference.prompts()[0];
        assert!(
            prompt.contains("13. [importance 3] The owner's dog is named Pochi"),
            "the older relevant memory is listed in the prompt: {prompt}"
        );
        assert!(
            !prompt.contains("unrelated note 0 about the weather"),
            "the bounded prompt does not take every older memory"
        );
    }

    #[tokio::test]
    async fn an_oversized_newest_turn_is_retained_rather_than_dropped() {
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "s", "memories": []}"#,
        ))]);
        let candidate = candidate_text(
            RawId::new(),
            &format!(
                "marker-newest-experience {}",
                "x".repeat(FORMATION_PROMPT_CHAR_BUDGET)
            ),
        );
        let _ = form_experience(&repository, &inference, &scrubber(), candidate)
            .await
            .unwrap();
        let prompt = &inference.prompts()[0];
        assert!(
            prompt.contains("marker-newest-experience"),
            "an oversized newest turn must still reach the provider, not be silently dropped"
        );
    }

    #[tokio::test]
    async fn the_prompt_drops_whole_older_turns_under_the_budget() {
        let companion = RawId::new();
        let repository = FakeLearningRepository::new();
        let inference = ScriptedInference::new(vec![Ok(String::from(
            r#"{"summary": "s", "memories": []}"#,
        ))]);
        let transcript: Vec<ExperienceTurn> = (0..10)
            .map(|index| ExperienceTurn {
                role: ExperienceRole::Owner,
                text: format!("turn-{index}-{}", "y".repeat(900)),
                at: None,
            })
            .collect();
        let candidate = ExperienceCandidate {
            companion,
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: RawId::new(),
                end: RawId::new(),
            },
            sources: Vec::new(),
            transcript,
            at: WallClockWithTz::now(),
        };
        let _ = form_experience(&repository, &inference, &scrubber(), candidate)
            .await
            .unwrap();
        let prompt = &inference.prompts()[0];
        assert!(
            prompt.contains("turn-9-"),
            "the newest turn is retained: {prompt}"
        );
        assert!(
            !prompt.contains("turn-0-"),
            "whole older turns are dropped once the budget is reached: {prompt}"
        );
        assert!(
            prompt.chars().count() <= FORMATION_PROMPT_CHAR_BUDGET + 200,
            "the assembled prompt stays near the budget: {}",
            prompt.chars().count()
        );
    }
}
