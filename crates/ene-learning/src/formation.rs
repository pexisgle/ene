//! Experience formation: compress one experience into Summary evidence and
//! propose the Memory entries it grounds.
//!
//! The semantic decision belongs to the Learning pass, not to fixed rules:
//! the caller supplies the Experience, this module asks the supplied
//! [`LearningInference`] to judge it, and the parsed answer is committed
//! through [`LearningRepository`]. Everything the model sees and everything
//! it produces passes through [`SecretScrubber`] first, so registered secret
//! material can neither reach the model context nor be stored as a Summary or
//! Memory.
//!
//! Stage 3 forms and updates companion-scoped memories. Existing memories
//! are shown to the model with positional references so repeated information
//! can reinforce, refine, or integrate an existing recognition instead of
//! creating a duplicate, and so a correction can name the Memory it changes.
//! Candidate selection always includes the newest memories plus the older
//! ones that overlap the new experience, bounded by [`FORMATION_SCAN_LIMIT`],
//! so a correction can still reach a relevant Memory outside the newest
//! window without an embedding or index.
//!
//! The model answer must decide the semantic schema for every entry: an
//! action, an existing target for updates and forgets, the change kind for an
//! update, and a known temporal meaning when supplied. A missing or unknown
//! value is not guessed at; the whole answer is
//! [`FormationDecision::DeferredForContext`] and nothing is stored. Every
//! update carries the revision it was judged from and commits through
//! compare-before-commit: a stale target is reported, never overwritten.

use ene_primitive::{RawId, WallClockWithTz};
use serde::Deserialize;
use thiserror::Error;

use ene_credential::{ScrubbedText, SecretScrubError, SecretScrubber};

use crate::identity::{MemoryId, MemoryRevision, SourceRangeRef, SummaryId};
use crate::memory::{ChangeKind, Importance, Memory, TemporalMeaning};
use crate::repository::{
    LearningRepository, LearningTechnicalError, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryTarget,
};
use crate::scope::LearningScope;
use crate::summary::SummaryRecord;

/// Most Memory changes one formation pass accepts.
///
/// The prompt states this cap. An answer proposing more is undecidable as a
/// whole and deferred rather than truncated, so no entry is silently dropped.
pub const MAX_FORMATION_CHANGES: usize = 5;

/// Cap on the turns read into one formation prompt.
pub const MAX_FORMATION_TURNS: usize = 24;

/// Most current memories one formation reads before selecting candidates.
///
/// The read is a bounded newest-first scan: an environment with more
/// memories than this needs an indexed or embedding selection instead.
pub const FORMATION_SCAN_LIMIT: u64 = 200;

/// Newest current memories always offered to the model.
const RECENT_MEMORY_LIMIT: usize = 12;

/// Older current memories offered when they overlap the new experience.
const RELEVANT_MEMORY_LIMIT: usize = 8;

/// Total existing memories shown to the model in one prompt.
const EXISTING_MEMORY_LIMIT: usize = RECENT_MEMORY_LIMIT + RELEVANT_MEMORY_LIMIT;

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

/// Which side of the conversation produced one Experience turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperienceRole {
    Owner,
    Companion,
}

/// One transient turn of the Experience being judged.
///
/// The text is source material for this formation only; it is never stored by
/// this crate. It is redacted from [`core::fmt::Debug`] because it may quote
/// owner speech.
#[derive(Clone, PartialEq, Eq)]
pub struct ExperienceTurn {
    pub role: ExperienceRole,
    pub text: String,
}

impl core::fmt::Debug for ExperienceTurn {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ExperienceTurn")
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .finish()
    }
}

/// Stage 3 dialogue correspondence of one Experience.
///
/// Carries the parts of the `ProposeExperienceCandidate` boundary a dialogue
/// formation needs and that an in-memory queue must not lose while the pass
/// is pending: the Client and round the turn belonged to, and the presence
/// generation anchoring continuity within one Host run. Cross-domain
/// identities stay opaque [`RawId`]s and are never converted here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExperienceCorrespondence {
    /// Client that accepted the round, when known.
    pub client: Option<RawId>,
    /// Round the completed turn belonged to, when known.
    pub round: Option<RawId>,
    /// Presence generation at acceptance; continuity within one Host run.
    pub generation: Option<u64>,
}

/// One experience proposed for formation.
///
/// `source` references the retained History the transcript was read from; the
/// transcript itself is transient and is not copied into durable Learning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceCandidate {
    /// Companion whose Experience this is; the only scope the formation can
    /// produce.
    pub companion: RawId,
    pub source: SourceRangeRef,
    pub transcript: Vec<ExperienceTurn>,
    pub at: WallClockWithTz,
    /// Client / round / continuity correspondence confirmed when the
    /// Experience was proposed.
    pub correspondence: ExperienceCorrespondence,
}

/// One change the formation applied or rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormationChange {
    Applied {
        memory: MemoryId,
        revision: MemoryRevision,
    },
    Rejected {
        memory: MemoryId,
        reason: ChangeRejection,
    },
}

/// Why a proposed change was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeRejection {
    StaleTarget,
    MissingTarget,
    ScopeMismatch,
    AlreadyExists,
    RevisionExhausted,
}

/// What one formation pass decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormationDecision {
    /// Summary evidence was stored and at least one Memory change applied.
    Formed {
        summary: SummaryId,
        changes: Vec<FormationChange>,
    },
    /// Every proposed change lost its compare-before-commit, so no Summary
    /// evidence was stored and no newer recognition was touched.
    RejectedAsStale { changes: Vec<FormationChange> },
    /// The model judged the experience not worth keeping; nothing was stored.
    DeclinedAsNoEndValue,
    /// The answer could not be interpreted as the semantic schema, or
    /// inference declined; nothing was stored, and no entry of a partly
    /// undecidable answer is committed or invented.
    DeferredForContext,
}

/// The model boundary used to judge one Experience.
///
/// Kept as a port so this crate does not depend on inference or permission
/// crates: the Host supplies an implementation through the inference boundary
/// with its own consumer and purpose. The prompt carries the credential-set
/// premise it was scrubbed under, so the send claim can refuse a prompt that
/// predates a credential registration.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait LearningInference: Send + Sync {
    async fn infer(&self, prompt: ScrubbedText) -> Result<String, LearningInferenceError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LearningInferenceError {
    /// Nothing was sent, or consent moved before adoption.
    #[error("learning inference was declined")]
    Declined,
    /// The provider or transport failed; no judgment exists.
    #[error("learning inference unavailable: {reason}")]
    Unavailable {
        /// Provider-class cause. Never prompt or output text.
        reason: String,
    },
}

/// Forms one Experience: judge it, then commit Summary evidence and the
/// Memory changes it grounds.
///
/// The inference call happens outside any storage transaction. Each change is
/// committed through the repository's compare-before-commit boundary, so a
/// concurrent change to the same Memory is rejected rather than overwritten.
///
/// # Errors
///
/// [`LearningTechnicalError`] reports storage or inference infrastructure
/// failure; a model answer that cannot be interpreted is the domain outcome
/// [`FormationDecision::DeferredForContext`], never an error.
pub async fn form_experience(
    repository: &impl LearningRepository,
    inference: &impl LearningInference,
    scrubber: &impl SecretScrubber,
    candidate: ExperienceCandidate,
) -> Result<FormationDecision, LearningTechnicalError> {
    let scope = LearningScope::companion(candidate.companion);
    let scanned = repository
        .list_current_memories(candidate.companion, FORMATION_SCAN_LIMIT)
        .await?;
    let existing = select_existing(scanned, &candidate.transcript);
    let prompt = build_prompt(&existing, &candidate, scrubber).await?;
    let answer = match inference.infer(prompt).await {
        Ok(answer) => answer,
        Err(LearningInferenceError::Declined) => {
            return Ok(FormationDecision::DeferredForContext);
        }
        Err(LearningInferenceError::Unavailable { reason }) => {
            return Err(LearningTechnicalError::InferenceUnavailable { reason });
        }
    };
    let Some(answer) = parse_answer(&answer) else {
        return Ok(FormationDecision::DeferredForContext);
    };
    let Some(summary_text) = answer.summary else {
        // A formation with no compressed evidence has no grounds to attach to
        // a Memory; refusing is safer than storing an unexplained recognition.
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_text = scrubber
        .scrub(&summary_text)
        .await
        .map_err(secret_boundary_failure)?;
    if summary_text.text.trim().is_empty() || answer.memories.is_empty() {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    }
    // Resolve every entry before the first commit. One entry the schema
    // cannot interpret makes the whole answer undecidable: committing only
    // the other entries would reconstruct the model's meaning from a partly
    // unreadable answer.
    let Some(proposals) = resolve_model_memories(answer.memories, &existing) else {
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_id = SummaryId::generate();
    let summary = SummaryRecord {
        id: summary_id,
        scope,
        content: summary_text.text.trim().to_owned(),
        source: candidate.source,
        formed_at: candidate.at,
    };

    // Scrub every offered content before the first commit, so one premise
    // covers each durable piece this pass is about to write. A credential
    // registration between two pieces would otherwise let an earlier piece
    // carry the newly registered value into storage.
    let mut prepared = Vec::new();
    let mut changes = Vec::new();
    let mut applied = false;
    for proposal in proposals {
        let content = scrubber
            .scrub(&proposal.content)
            .await
            .map_err(secret_boundary_failure)?;
        if content.text.trim().is_empty() {
            // Resolution guarantees non-empty model or stored content, so an
            // empty scrub result cannot ground a Memory.
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
        let content = content.text.trim().to_owned();
        let outcome = repository
            .commit_memory_change(MemoryChangeCommit {
                summary: Some(summary.clone()),
                secret_premise,
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
        changes.push(match outcome {
            MemoryChangeOutcome::Committed { memory, revision } => {
                applied = true;
                FormationChange::Applied { memory, revision }
            }
            MemoryChangeOutcome::StaleTarget { memory, .. } => FormationChange::Rejected {
                memory,
                reason: ChangeRejection::StaleTarget,
            },
            MemoryChangeOutcome::MissingTarget { memory } => FormationChange::Rejected {
                memory,
                reason: ChangeRejection::MissingTarget,
            },
            MemoryChangeOutcome::ScopeMismatch { memory } => FormationChange::Rejected {
                memory,
                reason: ChangeRejection::ScopeMismatch,
            },
            MemoryChangeOutcome::AlreadyExists { memory } => FormationChange::Rejected {
                memory,
                reason: ChangeRejection::AlreadyExists,
            },
            MemoryChangeOutcome::RevisionExhausted { memory } => FormationChange::Rejected {
                memory,
                reason: ChangeRejection::RevisionExhausted,
            },
            MemoryChangeOutcome::StaleCredentialSet => {
                // The set moved after the scrub: the prepared content may
                // carry the newly registered value. Refuse the whole pass
                // instead of writing raw text; already committed pieces are
                // covered by the approval sweep.
                return Err(LearningTechnicalError::SecretBoundaryUnavailable {
                    reason: String::from("credential set moved during formation"),
                });
            }
        });
    }
    if applied {
        return Ok(FormationDecision::Formed {
            summary: summary_id,
            changes,
        });
    }
    Ok(FormationDecision::RejectedAsStale { changes })
}

async fn build_prompt(
    existing: &[Memory],
    candidate: &ExperienceCandidate,
    scrubber: &impl SecretScrubber,
) -> Result<ScrubbedText, LearningTechnicalError> {
    let mut prompt = String::from(PROMPT_PREAMBLE);
    let mut premises = Vec::new();
    prompt.push_str("\n\nExisting memories:\n");
    if existing.is_empty() {
        prompt.push_str("(none)\n");
    } else {
        for (position, memory) in existing.iter().enumerate() {
            let content = scrubber
                .scrub(&memory.content)
                .await
                .map_err(secret_boundary_failure)?;
            premises.push(content.credential_set);
            prompt.push_str(&format!(
                "{}. [importance {}] ",
                position + 1,
                memory.importance.as_u8()
            ));
            prompt.push_str(&content.text);
            prompt.push('\n');
        }
    }
    prompt.push_str("\nNew experience:\n");
    for turn in candidate.transcript.iter().take(MAX_FORMATION_TURNS) {
        let text = scrubber
            .scrub(&turn.text)
            .await
            .map_err(secret_boundary_failure)?;
        premises.push(text.credential_set);
        prompt.push_str(match turn.role {
            ExperienceRole::Owner => "Owner: ",
            ExperienceRole::Companion => "Companion: ",
        });
        prompt.push_str(&text.text);
        prompt.push('\n');
    }
    prompt.push('\n');
    prompt.push_str(PROMPT_SCHEMA);
    prompt.push_str(&format!(
        "\nReturn at most {MAX_FORMATION_CHANGES} memory entries; keep only the most important when more changes seem needed."
    ));
    let credential_set = match premises.into_iter().min() {
        Some(revision) => revision,
        // No scrubbed piece exists (empty transcript); still bind the prompt
        // to a current premise by scrubbing an empty string.
        None => {
            scrubber
                .scrub("")
                .await
                .map_err(secret_boundary_failure)?
                .credential_set
        }
    };
    Ok(ScrubbedText {
        text: prompt,
        credential_set,
    })
}

fn secret_boundary_failure(error: SecretScrubError) -> LearningTechnicalError {
    LearningTechnicalError::SecretBoundaryUnavailable {
        reason: error.to_string(),
    }
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
    let experience = transcript
        .iter()
        .take(MAX_FORMATION_TURNS)
        .map(|turn| turn.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let terms = crate::relevance::terms(&experience);
    let mut older: Vec<(usize, &Memory)> = scanned[RECENT_MEMORY_LIMIT..]
        .iter()
        .map(|memory| (crate::relevance::overlap(&terms, &memory.content), memory))
        .collect();
    // Stable sort: equal overlap keeps the repository's newest-first order.
    older.sort_by(|(left, _), (right, _)| right.cmp(left));
    selected.extend(
        older
            .into_iter()
            .take(RELEVANT_MEMORY_LIMIT)
            .map(|(_, memory)| memory.clone()),
    );
    selected
}

/// One model entry resolved against the memories the prompt listed.
struct ResolvedChange {
    target: MemoryTarget,
    change: ChangeKind,
    content: String,
    importance: Importance,
    temporal: TemporalMeaning,
}

/// Resolves every listed entry, or `None` when any entry cannot be decided.
///
/// An answer listing more than [`MAX_FORMATION_CHANGES`] entries is undecidable
/// as a whole: the pass cannot tell which changes the model would keep, so
/// nothing is committed instead of truncating the list.
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

/// Decides the semantic schema of one model entry.
///
/// Returns `None` for a missing or unknown action, a missing or invalid
/// target for an update/forget, a missing or unknown change kind for an
/// update, and a supplied but unknown temporal meaning. Fields that may
/// intentionally keep an existing value (update content, importance, and
/// temporal) default to the stored value; a new memory needs the model's
/// content and temporal decision because it has no stored value to keep.
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
                    // Omitting the content keeps the stored recognition: an
                    // update may reinforce without restating it.
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
                // Normal forgetting re-saves the recognition untouched; the
                // model cannot delete or rewrite it through this action.
                content: known.content.clone(),
                importance: known.importance,
                temporal: known.temporal,
            })
        }
    }
}

/// Resolves a 1-based prompt position to the listed memory it names.
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

/// Tolerant answer decoding: exactly one JSON object, optionally wrapped in
/// prose or a code fence, and only the known fields are read. A missing or
/// malformed object yields [`None`] so the caller stores nothing.
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
        ExperienceCandidate, ExperienceCorrespondence, ExperienceRole, ExperienceTurn,
        FormationChange, FormationDecision, LearningInferenceError, form_experience,
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
            transcript: turns
                .iter()
                .map(|(role, text)| ExperienceTurn {
                    role: match *role {
                        "owner" => ExperienceRole::Owner,
                        _ => ExperienceRole::Companion,
                    },
                    text: (*text).to_owned(),
                })
                .collect(),
            at: WallClockWithTz::now(),
            correspondence: ExperienceCorrespondence::default(),
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
        let FormationDecision::Formed { summary, changes } = decision else {
            panic!("a useful experience must form");
        };
        assert_eq!(changes.len(), 1);
        let FormationChange::Applied { memory, revision } = changes[0] else {
            panic!("the new memory must apply");
        };
        assert_eq!(revision, MemoryRevision::initial());
        let stored = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.content, "The owner likes jasmine tea.");
        assert_eq!(stored.scope, LearningScope::companion(companion));
        assert_eq!(stored.importance.as_u8(), 4);
        let evidence = repository.load_summary(summary).await.unwrap().unwrap();
        assert_eq!(evidence.content, "The owner likes jasmine tea.");
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
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
        ExperienceCandidate, ExperienceCorrespondence, ExperienceRole, ExperienceTurn,
        FormationChange, FormationDecision, MAX_FORMATION_CHANGES, form_experience,
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
            transcript: vec![ExperienceTurn {
                role: ExperienceRole::Owner,
                text: text.to_owned(),
            }],
            at: WallClockWithTz::now(),
            correspondence: ExperienceCorrespondence::default(),
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
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].content, "owner likes tea");
        assert_eq!(revisions[1].change, ChangeKind::Reinforced);
        assert_eq!(
            repository
                .load_current_memory(memory)
                .await
                .unwrap()
                .unwrap()
                .revision,
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
        let current = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.content, "owner prefers jasmine tea in the morning");
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
        assert_eq!(revisions.len(), 2);
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
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
        assert_eq!(revisions[1].change, ChangeKind::CorrectedInitiallyWrong);

        let (repository, memory, decision) = apply_update(
            r#"{"summary": "The situation changed.", "memories": [{"action": "update", "target": 1, "change": "changed_since", "content": "owner switched to coffee"}]}"#,
        )
        .await;
        assert!(matches!(decision, FormationDecision::Formed { .. }));
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
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
        let current = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert!(current.recall_suppressed, "recall is suppressed");
        assert_eq!(
            current.content, "owner likes tea",
            "normal forgetting never deletes content"
        );
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
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
        let current = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            current.importance.as_u8(),
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
        let FormationDecision::RejectedAsStale { changes } = decision else {
            panic!("a moved target must reject the stale formation, got {decision:?}");
        };
        assert!(matches!(
            changes.as_slice(),
            [FormationChange::Rejected { .. }]
        ));
        let current = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            current.content, "advanced by another formation",
            "the newer recognition is untouched"
        );
        assert_eq!(
            repository
                .list_memory_revisions(memory)
                .await
                .unwrap()
                .len(),
            2,
            "the stale change leaves no revision"
        );
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
            let current = repository
                .load_current_memory(memory)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(current.revision, revision, "no revision may advance");
            assert_eq!(
                repository
                    .list_memory_revisions(memory)
                    .await
                    .unwrap()
                    .len(),
                1,
                "no partial revision may remain"
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
        let current = repository
            .load_current_memory(memory)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.content, "The owner's dog is named Momo");
        assert_eq!(current.revision, MemoryRevision::from_u64(2));
        let revisions = repository.list_memory_revisions(memory).await.unwrap();
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
}
