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
//! Stage 3 forms new companion-scoped memories only; updating existing ones
//! arrives with the revision pass. Existing memories are still shown to the
//! model so it can decline to duplicate what is already known.

use ene_primitive::{RawId, WallClockWithTz};
use serde::Deserialize;
use thiserror::Error;

use crate::identity::{MemoryId, MemoryRevision, SourceRangeRef, SummaryId};
use crate::memory::{ChangeKind, Importance, Memory, TemporalMeaning};
use crate::repository::{
    LearningRepository, LearningTechnicalError, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryTarget,
};
use crate::scope::LearningScope;
use crate::summary::SummaryRecord;

/// Cap on Memories formed from one experience.
pub const MAX_FORMED_MEMORIES: usize = 5;

/// Cap on the turns read into one formation prompt.
pub const MAX_FORMATION_TURNS: usize = 24;

const EXISTING_MEMORY_LIMIT: u64 = 20;

const PROMPT_PREAMBLE: &str = "\
You are ene's learning formation pass for one companion.
Decide what, if anything, from the new experience below deserves to be kept as long-term Memory.
Rules:
- Answer with one JSON object and nothing else.
- Write a short summary of what happened and what it means for later understanding.
- Keep only information useful for understanding the owner, the companion, or their shared situations.
- Do not keep small talk, transient states, or raw conversation.
- Weigh an explicit request to remember (for example \"remember this\" or \"覚えておいて\") highly.
- Never keep credential values or secrets, even when asked to remember them.
- Do not duplicate an existing memory: omit information that is already known.
- Memory scope is always this companion; never claim shared or global scope.";

const PROMPT_SCHEMA: &str = "\
Answer exactly as:
{\"summary\": \"compressed evidence\", \"memories\": [{\"content\": \"...\", \"importance\": 3, \"temporal\": \"enduring\"}]}
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
    /// The model judged the experience not worth keeping; nothing was stored.
    DeclinedAsNoEndValue,
    /// The answer could not be interpreted or inference declined; nothing was
    /// stored and no partial decision is invented.
    DeferredForContext,
}

/// The model boundary used to judge one Experience.
///
/// Kept as a port so this crate does not depend on inference, permission, or
/// credential crates: the Host supplies an implementation through the
/// inference boundary with its own consumer and purpose.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait LearningInference: Send + Sync {
    async fn infer(&self, prompt: String) -> Result<String, LearningInferenceError>;
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

/// Redaction of registered secret values.
///
/// The Host owns the credential boundary; this crate only requires that a
/// value never reaches a prompt or a durable Learning row. Implementations
/// must return text with secret occurrences removed and must not expose the
/// secret itself.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait SecretScrubber: Send + Sync {
    async fn scrub(&self, text: &str) -> String;
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
    let existing = repository
        .list_current_memories(candidate.companion, EXISTING_MEMORY_LIMIT)
        .await?;
    let prompt = build_prompt(&existing, &candidate, scrubber).await;
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
    let summary_text = scrubber.scrub(&summary_text).await;
    if summary_text.trim().is_empty() || answer.memories.is_empty() {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    }
    let summary_id = SummaryId::generate();
    let summary = SummaryRecord {
        id: summary_id,
        scope,
        content: summary_text.trim().to_owned(),
        source: candidate.source,
        formed_at: candidate.at,
    };

    let mut changes = Vec::new();
    for proposed in answer.memories.into_iter().take(MAX_FORMED_MEMORIES) {
        let Some(content) = proposed.content else {
            continue;
        };
        let content = scrubber.scrub(&content).await;
        let content = content.trim().to_owned();
        if content.is_empty() {
            continue;
        }
        let memory = MemoryId::generate();
        let importance = Importance::clamped(
            proposed
                .importance
                .unwrap_or_else(|| Importance::default().as_u8()),
        );
        let temporal = parse_temporal(proposed.temporal.as_deref());
        let outcome = repository
            .commit_memory_change(MemoryChangeCommit {
                summary: Some(summary.clone()),
                change: MemoryChange {
                    target: MemoryTarget::New { id: memory },
                    scope,
                    content,
                    importance,
                    temporal,
                    change: ChangeKind::Initial,
                    recall_suppressed: false,
                    at: candidate.at,
                },
            })
            .await?;
        changes.push(match outcome {
            MemoryChangeOutcome::Committed { memory, revision } => {
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
        });
    }
    if changes.is_empty() {
        return Ok(FormationDecision::DeclinedAsNoEndValue);
    }
    Ok(FormationDecision::Formed {
        summary: summary_id,
        changes,
    })
}

async fn build_prompt(
    existing: &[Memory],
    candidate: &ExperienceCandidate,
    scrubber: &impl SecretScrubber,
) -> String {
    let mut prompt = String::from(PROMPT_PREAMBLE);
    prompt.push_str("\n\nExisting memories:\n");
    if existing.is_empty() {
        prompt.push_str("(none)\n");
    } else {
        for memory in existing {
            prompt.push_str("- ");
            prompt.push_str(&memory.content);
            prompt.push('\n');
        }
    }
    prompt.push_str("\nNew experience:\n");
    for turn in candidate.transcript.iter().take(MAX_FORMATION_TURNS) {
        let text = scrubber.scrub(&turn.text).await;
        prompt.push_str(match turn.role {
            ExperienceRole::Owner => "Owner: ",
            ExperienceRole::Companion => "Companion: ",
        });
        prompt.push_str(&text);
        prompt.push('\n');
    }
    prompt.push('\n');
    prompt.push_str(PROMPT_SCHEMA);
    prompt
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
    content: Option<String>,
    #[serde(default)]
    importance: Option<u8>,
    #[serde(default)]
    temporal: Option<String>,
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

fn parse_temporal(value: Option<&str>) -> TemporalMeaning {
    match value {
        Some("event") => TemporalMeaning::Event,
        _ => TemporalMeaning::Enduring,
    }
}

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, WallClockWithTz};

    use crate::formation::{
        ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationChange, FormationDecision,
        LearningInferenceError, form_experience,
    };
    use crate::identity::{ExperienceSourceKind, MemoryRevision, SourceRangeRef};
    use crate::repository::{LearningRepository, LearningTechnicalError};
    use crate::scope::LearningScope;
    use crate::test_support::{FakeLearningRepository, ReplacingScrubber, ScriptedInference};

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
        }
    }

    fn answer() -> String {
        String::from(
            r#"{"summary": "The owner likes jasmine tea.", "memories": [{"content": "The owner likes jasmine tea.", "importance": 4, "temporal": "enduring"}]}"#,
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
            r#"{"summary": "The owner shared a key: sk-secret.", "memories": [{"content": "The key is sk-secret.", "importance": 5}]}"#,
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
}
