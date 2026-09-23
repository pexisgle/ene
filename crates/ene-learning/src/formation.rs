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

use crate::identity::{LearningClaimRef, MemoryId, SourceRangeRef, SummaryId};
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
/// owner speech; `at` is displayable provenance, not a secret.
///
/// `at` is the source message's recorded wall-clock time with its creation
/// offset, kept so a relative date in the text stays bound to when it was
/// said. [`None`] means the source has no recorded time: the formation never
/// invents one, and the prompt renders the turn without a timestamp.
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
    /// Every History message identity the transcript was read from, in read
    /// order. The coarse [`Self::source`] range is the Summary's evidence
    /// reference; this ordered set is the formation's canonical provenance
    /// claim, so a deletion operation that covers any of these messages can
    /// associate an already-claimed formation with its interval. Values are
    /// identities, never bodies or hashes.
    pub sources: Vec<RawId>,
    pub transcript: Vec<ExperienceTurn>,
    pub at: WallClockWithTz,
}

/// What one formation pass decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormationDecision {
    /// Summary evidence was stored and at least one Memory change applied.
    Formed { summary: SummaryId },
    /// No proposed change was applied: every compare-before-commit lost, or
    /// the target was missing, out of scope, already present, or its
    /// revision exhausted. Nothing was stored and no newer recognition was
    /// touched.
    NoChangesApplied,
    /// The model judged the experience not worth keeping; nothing was stored.
    DeclinedAsNoEndValue,
    /// The answer could not be interpreted as the semantic schema, or
    /// inference declined; nothing was stored, and no entry of a partly
    /// undecidable answer is committed or invented.
    DeferredForContext,
}

/// The canonical provenance of one formation pass, carried to the inference
/// boundary.
///
/// The identities are the same opaque correlation values the repository and
/// the Summary use: the transcript message identities and the current Memory
/// identities the prompt read. They are never bodies or hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningInferencePremise {
    pub data_use: Vec<RawId>,
}

impl LearningInferencePremise {
    #[must_use]
    pub fn new(data_use: Vec<RawId>) -> Self {
        Self { data_use }
    }

    #[must_use]
    pub fn data_use(&self) -> &[RawId] {
        &self.data_use
    }
}

/// One inference answer together with the durable claim it ran under.
///
/// The claim is the opaque identity of the provider attempt (the inference
/// ticket). The formation carries it into every commit so the store can
/// refuse a delayed formation whose provenance was associated with a deletion
/// operation, even after that operation completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningInferenceAnswer {
    pub answer: String,
    pub claim: LearningClaimRef,
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
        .list_current_memories(candidate.companion, None, FORMATION_SCAN_LIMIT)
        .await?;
    let existing = select_existing(scanned, &candidate.transcript);
    let prompt = build_prompt(&existing, &candidate, scrubber).await?;
    // The claim's provenance is exactly what the prompt read: the transcript
    // messages pinned at reply completion and the current Memory identities
    // offered to the model, in prompt order. It rides the provider claim, so
    // a condition that committed first holds the send, and a deletion
    // admission can associate this formation with its interval.
    let mut data_use = candidate.sources.clone();
    data_use.extend(existing.iter().map(|memory| memory.id.as_raw()));
    let inferred = match inference
        .infer(LearningInferencePremise::new(data_use), prompt)
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
        // A formation with no compressed evidence has no grounds to attach to
        // a Memory; refusing is safer than storing an unexplained recognition.
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_text = scrubber
        .scrub(&summary_text)
        .await
        .map_err(secret_boundary_failure)?;
    if summary_text.text().trim().is_empty() || answer.memories.is_empty() {
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
        content: summary_text.text().trim().to_owned(),
        source: candidate.source,
        formed_at: candidate.at,
    };

    // Scrub every offered content before the first commit, so one premise
    // covers each durable piece this pass is about to write. A credential
    // registration between two pieces would otherwise let an earlier piece
    // carry the newly registered value into storage.
    let mut prepared = Vec::new();
    let mut applied = false;
    for proposal in proposals {
        let content = scrubber
            .scrub(&proposal.content)
            .await
            .map_err(secret_boundary_failure)?;
        if content.text().trim().is_empty() {
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
                // The set moved after the scrub: the prepared content may
                // carry the newly registered value. Refuse the whole pass
                // instead of writing raw text; already committed pieces are
                // covered by the approval sweep.
                return Err(LearningTechnicalError::SecretBoundaryUnavailable {
                    reason: String::from("credential set moved during formation"),
                });
            }
            // Stale / missing / scope / duplicate / exhausted rejections leave
            // `applied` false; the decision reports that nothing applied.
            _ => {}
        }
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
            premises.push(content.credential_set());
            prompt.push_str(&format!(
                "{}. [importance {}] ",
                position + 1,
                memory.importance.as_u8()
            ));
            prompt.push_str(content.text());
            prompt.push('\n');
        }
    }
    prompt.push_str("\nNew experience:\n");
    for turn in candidate.transcript.iter().take(MAX_FORMATION_TURNS) {
        let text = scrubber
            .scrub(&turn.text)
            .await
            .map_err(secret_boundary_failure)?;
        premises.push(text.credential_set());
        prompt.push_str(match turn.role {
            ExperienceRole::Owner => "Owner",
            ExperienceRole::Companion => "Companion",
        });
        // The source time stays attached to the turn it belongs to; an
        // unknown time is omitted rather than guessed from the others.
        if let Some(at) = turn.at {
            prompt.push_str(&format!(" [{}]", at.to_rfc3339()));
        }
        prompt.push_str(": ");
        prompt.push_str(text.text());
        prompt.push('\n');
    }
    prompt.push('\n');
    prompt.push_str(PROMPT_SCHEMA);
    prompt.push_str(&format!(
        "\nReturn at most {MAX_FORMATION_CHANGES} memory entries; keep only the most important when more changes seem needed."
    ));
    // Assembly can introduce a value across fragment boundaries or in
    // formatting. Only the credential owner can mint the final proof.
    let scrubbed = scrubber
        .scrub(&prompt)
        .await
        .map_err(secret_boundary_failure)?;
    Ok(match premises.into_iter().min() {
        Some(prior) => scrubbed.with_oldest_premise(prior),
        None => scrubbed,
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
