//! In-crate fakes for Learning tests.
//!
//! These stand in for the durable repository and the Host inference adapter so
//! formation and retrieval semantics can be tested without a database or a
//! provider. They model domain behavior (compare-before-commit, suppression)
//! rather than SQL.

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_primitive::RawId;

use crate::{
    CredentialSetRevision, LearningInference, LearningInferenceError, LearningRepository,
    LearningScope, LearningTechnicalError, Memory, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryId, MemoryRevision, MemoryRevisionRecord, MemoryTarget,
    ScrubbedText, SecretScrubber, SummaryId, SummaryRecord,
};

#[derive(Default)]
pub(crate) struct FakeLearningRepository {
    memories: Mutex<Vec<Memory>>,
    revisions: Mutex<Vec<MemoryRevisionRecord>>,
    summaries: Mutex<Vec<SummaryRecord>>,
}

impl FakeLearningRepository {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn current(&self) -> Vec<Memory> {
        self.memories.lock().expect("fake memory lock").clone()
    }

    pub(crate) fn summaries(&self) -> Vec<SummaryRecord> {
        self.summaries.lock().expect("fake summary lock").clone()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the repository contract"
)]
impl LearningRepository for FakeLearningRepository {
    async fn commit_memory_change(
        &self,
        commit: MemoryChangeCommit,
    ) -> Result<MemoryChangeOutcome, LearningTechnicalError> {
        // Identity check first: a reused Summary id may only carry the same
        // payload. The record itself is inserted only when a change commits,
        // matching the store's insert-then-rollback transaction.
        if let Some(summary) = &commit.summary {
            let summaries = self.summaries.lock().expect("fake summary lock");
            if let Some(stored) = summaries.iter().find(|stored| stored.id == summary.id)
                && stored != summary
            {
                return Err(LearningTechnicalError::SummaryIdentityConflict {
                    summary: summary.id,
                });
            }
        }
        let change = commit.change;
        let summary = commit.summary.as_ref().map(|summary| summary.id);
        let mut memories = self.memories.lock().expect("fake memory lock");
        let outcome = match change.target {
            MemoryTarget::New { id } => {
                if memories.iter().any(|memory| memory.id == id) {
                    MemoryChangeOutcome::AlreadyExists { memory: id }
                } else {
                    let revision = MemoryRevision::initial();
                    memories.push(memory_of(id, revision, &change));
                    MemoryChangeOutcome::Committed {
                        memory: id,
                        revision,
                    }
                }
            }
            MemoryTarget::Existing {
                id,
                expected_revision,
            } => match memories.iter_mut().find(|memory| memory.id == id) {
                None => MemoryChangeOutcome::MissingTarget { memory: id },
                Some(current) if current.scope != change.scope => {
                    MemoryChangeOutcome::ScopeMismatch { memory: id }
                }
                Some(current) if current.revision != expected_revision => {
                    MemoryChangeOutcome::StaleTarget {
                        memory: id,
                        current: current.revision,
                    }
                }
                Some(current) => match current.revision.checked_next() {
                    None => MemoryChangeOutcome::RevisionExhausted { memory: id },
                    Some(next) => {
                        *current = memory_of(id, next, &change);
                        MemoryChangeOutcome::Committed {
                            memory: id,
                            revision: next,
                        }
                    }
                },
            },
        };
        if let MemoryChangeOutcome::Committed { memory, revision } = outcome {
            if let Some(summary) = &commit.summary {
                let mut summaries = self.summaries.lock().expect("fake summary lock");
                if !summaries.iter().any(|stored| stored.id == summary.id) {
                    summaries.push(summary.clone());
                }
            }
            self.revisions
                .lock()
                .expect("fake revision lock")
                .push(MemoryRevisionRecord {
                    memory,
                    revision,
                    scope: change.scope,
                    content: change.content.clone(),
                    importance: change.importance,
                    temporal: change.temporal,
                    change: change.change,
                    recall_suppressed: change.recall_suppressed,
                    summary,
                    at: change.at,
                });
        }
        Ok(outcome)
    }

    async fn list_current_memories(
        &self,
        companion: RawId,
        after: Option<MemoryId>,
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError> {
        let memories = self.memories.lock().expect("fake memory lock");
        let cap = usize::try_from(limit).unwrap_or(usize::MAX);
        let scoped = memories
            .iter()
            .rev()
            .filter(|memory| memory.scope == LearningScope::companion(companion));
        let page: Vec<Memory> = match after {
            // A cursor the fake does not hold yields no page, matching the
            // store's unknown-cursor behavior.
            Some(after) => scoped
                .skip_while(|memory| memory.id != after)
                .skip(1)
                .take(cap)
                .cloned()
                .collect(),
            None => scoped.take(cap).cloned().collect(),
        };
        Ok(page)
    }

    async fn list_memory_revisions(
        &self,
        memory: MemoryId,
    ) -> Result<Vec<MemoryRevisionRecord>, LearningTechnicalError> {
        Ok(self
            .revisions
            .lock()
            .expect("fake revision lock")
            .iter()
            .filter(|revision| revision.memory == memory)
            .cloned()
            .collect())
    }

    async fn load_summary(
        &self,
        summary: SummaryId,
    ) -> Result<Option<SummaryRecord>, LearningTechnicalError> {
        Ok(self
            .summaries
            .lock()
            .expect("fake summary lock")
            .iter()
            .find(|stored| stored.id == summary)
            .cloned())
    }
}

fn memory_of(id: MemoryId, revision: MemoryRevision, change: &MemoryChange) -> Memory {
    Memory {
        id,
        revision,
        scope: change.scope,
        content: change.content.clone(),
        importance: change.importance,
        temporal: change.temporal,
        recall_suppressed: change.recall_suppressed,
        updated_at: change.at,
    }
}

pub(crate) struct ScriptedInference {
    answers: Mutex<VecDeque<Result<String, LearningInferenceError>>>,
    prompts: Mutex<Vec<String>>,
}

impl ScriptedInference {
    pub(crate) fn new(answers: Vec<Result<String, LearningInferenceError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn prompts(&self) -> Vec<String> {
        self.prompts.lock().expect("fake prompt lock").clone()
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the inference port"
)]
impl LearningInference for ScriptedInference {
    async fn infer(&self, prompt: ScrubbedText) -> Result<String, LearningInferenceError> {
        self.prompts
            .lock()
            .expect("fake prompt lock")
            .push(prompt.text);
        self.answers
            .lock()
            .expect("fake answer lock")
            .pop_front()
            .unwrap_or(Err(LearningInferenceError::Declined))
    }
}

pub(crate) struct ReplacingScrubber {
    pub(crate) from: String,
    pub(crate) to: String,
}

impl ReplacingScrubber {
    pub(crate) fn new(from: &str, to: &str) -> Self {
        Self {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the scrubber contract"
)]
impl SecretScrubber for ReplacingScrubber {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, crate::SecretScrubError> {
        Ok(ScrubbedText {
            text: text.replace(&self.from, &self.to),
            credential_set: CredentialSetRevision::initial(),
        })
    }
}

/// A scrubber that can never prove absence, modelling an unreadable
/// credential registry or bearer.
pub(crate) struct FailingScrubber;

#[expect(
    clippy::unused_async_trait_impl,
    reason = "in-test fake; async matches the scrubber contract"
)]
impl SecretScrubber for FailingScrubber {
    async fn scrub(&self, _text: &str) -> Result<ScrubbedText, crate::SecretScrubError> {
        Err(crate::SecretScrubError::RegistryUnavailable)
    }
}

/// Seeds one memory directly, for tests that need an existing target.
pub(crate) async fn seed_memory(
    repository: &FakeLearningRepository,
    companion: RawId,
    content: &str,
) -> (MemoryId, MemoryRevision) {
    let id = MemoryId::generate();
    let outcome = repository
        .commit_memory_change(MemoryChangeCommit {
            summary: None,
            secret_premise: None,
            change: MemoryChange {
                target: MemoryTarget::New { id },
                scope: LearningScope::companion(companion),
                content: content.to_owned(),
                importance: crate::Importance::default(),
                temporal: crate::TemporalMeaning::Enduring,
                change: crate::ChangeKind::Initial,
                recall_suppressed: false,
                at: ene_primitive::WallClockWithTz::now(),
            },
        })
        .await
        .expect("the seeded memory must commit");
    let MemoryChangeOutcome::Committed { revision, .. } = outcome else {
        panic!("the seed must commit");
    };
    (id, revision)
}

/// Suppresses recall of one seeded memory through the normal change path.
pub(crate) async fn forget_memory(
    repository: &FakeLearningRepository,
    companion: RawId,
    memory: MemoryId,
    expected: MemoryRevision,
    content: &str,
) {
    drop(
        repository
            .commit_memory_change(MemoryChangeCommit {
                summary: None,
                secret_premise: None,
                change: MemoryChange {
                    target: MemoryTarget::Existing {
                        id: memory,
                        expected_revision: expected,
                    },
                    scope: LearningScope::companion(companion),
                    content: content.to_owned(),
                    importance: crate::Importance::default(),
                    temporal: crate::TemporalMeaning::Enduring,
                    change: crate::ChangeKind::Forgotten,
                    recall_suppressed: true,
                    at: ene_primitive::WallClockWithTz::now(),
                },
            })
            .await,
    );
}

/// An inference whose answer races a concurrent change to `advance` before
/// the formation commits, so the expected revision read for the prompt is
/// already stale.
pub(crate) struct RacingInference<'a> {
    repository: &'a FakeLearningRepository,
    companion: RawId,
    advance: MemoryId,
    expected: MemoryRevision,
    content: String,
    answer: String,
}

impl<'a> RacingInference<'a> {
    pub(crate) fn new(
        repository: &'a FakeLearningRepository,
        companion: RawId,
        advance: MemoryId,
        expected: MemoryRevision,
        answer: &str,
    ) -> Self {
        Self {
            repository,
            companion,
            advance,
            expected,
            content: String::from("advanced by another formation"),
            answer: answer.to_owned(),
        }
    }
}

impl LearningInference for RacingInference<'_> {
    async fn infer(&self, _prompt: ScrubbedText) -> Result<String, LearningInferenceError> {
        drop(
            self.repository
                .commit_memory_change(MemoryChangeCommit {
                    summary: None,
                    secret_premise: None,
                    change: MemoryChange {
                        target: MemoryTarget::Existing {
                            id: self.advance,
                            expected_revision: self.expected,
                        },
                        scope: LearningScope::companion(self.companion),
                        content: self.content.clone(),
                        importance: crate::Importance::default(),
                        temporal: crate::TemporalMeaning::Enduring,
                        change: crate::ChangeKind::Reinforced,
                        recall_suppressed: false,
                        at: ene_primitive::WallClockWithTz::now(),
                    },
                })
                .await,
        );
        Ok(self.answer.clone())
    }
}
