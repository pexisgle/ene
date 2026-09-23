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

pub const FORMATION_SCAN_LIMIT: u64 = 200;

const RECENT_MEMORY_LIMIT: usize = 12;

const RELEVANT_MEMORY_LIMIT: usize = 8;

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
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningInferenceAnswer {
    pub answer: String,
    pub claim: LearningClaimRef,
}

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
    let prompt = build_prompt(&existing, &candidate, scrubber).await?;
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
        return Ok(FormationDecision::DeferredForContext);
    };
    let summary_text = scrubber
        .scrub(&summary_text)
        .await
        .map_err(secret_boundary_failure)?;
    if summary_text.text().trim().is_empty() || answer.memories.is_empty() {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    }
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

    let mut prepared = Vec::new();
    let mut applied = false;
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
