//! Assembly regressions for the sealed scrub boundary (K-C.1 / #1530).
//!
//! The production dialogue assembler combines scrubbed fragments, headers,
//! and the owner input into one prompt. Only the credential-owned boundary
//! may mint the final proof, and the premise must stay the oldest of the
//! preparation pieces and the final scrub. These tests use the concrete
//! [`ene_credential::CredentialScrubber`] with fixture repositories and
//! stores; there is no public constructor to bypass it with.

use crate::dialogue::assemble_dialogue_input;
use crate::{CompanionId, HistoryMessage, HistoryRepository, HistoryRole};
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialScrubber, CredentialSetRevision,
    CredentialStore, CredentialTechnicalError,
};
use ene_learning::{LearningRepository, RecalledMemory};
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};
use std::sync::Mutex;

const ALPHA: &str = "sk-registered-alpha-value";
const LATE: &str = "sk-mid-assembly-registered-value";

/// Fixture registry: a scripted revision read by the credential boundary.
/// Armed actions fire on the nth boundary read *from arming time*, so a test
/// can move the credential set exactly between two passes of one assembly
/// and observe the move through the registry itself.
struct FixtureRegistry {
    alpha: CredentialRef,
    late: CredentialRef,
    refs: Mutex<Vec<CredentialRef>>,
    revision: Mutex<CredentialSetRevision>,
    /// (absolute read n, revision): after the nth `list_refs` call the late
    /// credential joins the registry and the revision moves, modelling an
    /// approval that registers a credential mid-assembly.
    expand_on_refs_read: Mutex<Option<(u32, CredentialSetRevision)>>,
    /// (absolute read n, revision): moves the revision on the nth revision
    /// read, modelling an approval that sweeps the set mid-assembly.
    advance_on_read: Mutex<Option<(u32, CredentialSetRevision)>>,
    refs_reads: Mutex<u32>,
    revision_reads: Mutex<u32>,
}

impl FixtureRegistry {
    fn new(revision: u64) -> Self {
        let alpha = CredentialRef::new("acme", "main").expect("valid fixture ref");
        let late = CredentialRef::new("acme", "late").expect("valid fixture ref");
        Self {
            refs: Mutex::new(vec![alpha.clone()]),
            alpha,
            late,
            revision: Mutex::new(CredentialSetRevision::from_u64(revision)),
            expand_on_refs_read: Mutex::new(None),
            advance_on_read: Mutex::new(None),
            refs_reads: Mutex::new(0),
            revision_reads: Mutex::new(0),
        }
    }

    /// Arms the expansion to apply on the `nth` `list_refs` call from now.
    fn expand_on_refs_read(&self, nth: u32, revision: u64) {
        let baseline = *self.refs_reads.lock().expect("fixture lock");
        *self.expand_on_refs_read.lock().expect("fixture lock") =
            Some((baseline + nth, CredentialSetRevision::from_u64(revision)));
    }

    /// Arms the revision to move to `revision` on the `nth` revision read
    /// from now.
    fn advance_on_read(&self, nth: u32, revision: u64) {
        let baseline = *self.revision_reads.lock().expect("fixture lock");
        *self.advance_on_read.lock().expect("fixture lock") =
            Some((baseline + nth, CredentialSetRevision::from_u64(revision)));
    }

    fn current(&self) -> CredentialSetRevision {
        *self.revision.lock().expect("fixture lock")
    }
}

impl CredentialRefRepository for FixtureRegistry {
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        {
            let mut reads = self.refs_reads.lock().expect("fixture lock");
            *reads += 1;
            if let Some((nth, revision)) = *self.expand_on_refs_read.lock().expect("fixture lock")
                && *reads == nth
            {
                self.refs
                    .lock()
                    .expect("fixture lock")
                    .push(self.late.clone());
                *self.revision.lock().expect("fixture lock") = revision;
            }
        }
        Ok(self.refs.lock().expect("fixture lock").clone())
    }
}

impl ene_credential::CredentialSetRepository for FixtureRegistry {
    async fn current_set_revision(
        &self,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
        let mut reads = self.revision_reads.lock().expect("fixture lock");
        *reads += 1;
        if let Some((nth, revision)) = *self.advance_on_read.lock().expect("fixture lock")
            && *reads == nth
        {
            *self.revision.lock().expect("fixture lock") = revision;
        }
        Ok(self.current())
    }
}

struct FixtureStore {
    alpha: CredentialRef,
    late: CredentialRef,
}

impl FixtureStore {
    fn for_refs(registry: &FixtureRegistry) -> Self {
        Self {
            alpha: registry.alpha.clone(),
            late: registry.late.clone(),
        }
    }
}

impl CredentialStore for FixtureStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        if cred == &self.alpha {
            return Ok(f(ALPHA));
        }
        if cred == &self.late {
            return Ok(f(LATE));
        }
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: String::from("unknown fixture ref"),
        })
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        cred == &self.alpha || cred == &self.late
    }

    fn put(&self, cred: &CredentialRef, _secret: &str) -> Result<(), CredentialTechnicalError> {
        let id = cred.id();
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!("{id}: fixture store does not accept serving-time put"),
        })
    }
}

fn history_item(text: &str) -> HistoryMessage {
    HistoryMessage {
        id: RawId::new(),
        companion: CompanionId::from_raw(RawId::new()),
        round: RawId::new(),
        role: HistoryRole::Owner,
        text: text.to_owned(),
        lang: String::from("en"),
        at: WallClockWithTz::now(),
        presence_generation: PresenceGeneration::first(),
        command_id: None,
        round_wire: None,
        round_intent: None,
        incarnation: None,
        local_id: None,
    }
}

/// History fixture returning the same window on every read.
struct FixedHistory<'a>(&'a [HistoryMessage]);

impl HistoryRepository for FixedHistory<'_> {
    async fn append_message(
        &self,
        _cmd: crate::AppendHistoryCommand,
    ) -> Result<crate::HistoryAppendOutcome, crate::CompanionTechnicalError> {
        Err(crate::CompanionTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn append_reply_with_undelivered(
        &self,
        _cmd: crate::AppendHistoryCommand,
        _register_unpresented: bool,
        _inference_claim: Option<RawId>,
    ) -> Result<
        (crate::HistoryAppendOutcome, Option<crate::UndeliveredRef>),
        crate::CompanionTechnicalError,
    > {
        Err(crate::CompanionTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn load_timeline(
        &self,
        _companion: CompanionId,
        _since: Option<WallClockWithTz>,
        _round: Option<RawId>,
        _limit: u64,
    ) -> Result<Vec<HistoryMessage>, crate::CompanionTechnicalError> {
        Err(crate::CompanionTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn lookup_command(
        &self,
        _companion: CompanionId,
        _command: &crate::CommandId,
    ) -> Result<Option<HistoryMessage>, crate::CompanionTechnicalError> {
        Err(crate::CompanionTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn load_message(
        &self,
        _message: RawId,
    ) -> Result<Option<HistoryMessage>, crate::CompanionTechnicalError> {
        Err(crate::CompanionTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn load_recent_timeline(
        &self,
        _companion: CompanionId,
        _limit: u64,
    ) -> Result<Vec<HistoryMessage>, crate::CompanionTechnicalError> {
        Ok(self.0.to_vec())
    }
}

/// Learning fixture whose recalled memories resolve through the real
/// `recall` ranking path via `recall_candidates`, so memory content actually
/// reaches the assembled prompt instead of being swallowed by the fixture.
struct FixedLearning<'a>(&'a [RecalledMemory]);

impl LearningRepository for FixedLearning<'_> {
    async fn commit_memory_change(
        &self,
        _commit: ene_learning::MemoryChangeCommit,
    ) -> Result<ene_learning::MemoryChangeOutcome, ene_learning::LearningTechnicalError> {
        Err(ene_learning::LearningTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn list_current_memories(
        &self,
        _companion: RawId,
        _after: Option<ene_learning::MemoryId>,
        _limit: u64,
    ) -> Result<Vec<ene_learning::Memory>, ene_learning::LearningTechnicalError> {
        Err(ene_learning::LearningTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn list_memory_revisions(
        &self,
        _memory: ene_learning::MemoryId,
        _after: Option<ene_learning::MemoryRevision>,
        _limit: u64,
    ) -> Result<Vec<ene_learning::MemoryRevisionRecord>, ene_learning::LearningTechnicalError> {
        Err(ene_learning::LearningTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }

    async fn recall_candidates(
        &self,
        companion: RawId,
        _terms: &[String],
        _limit: u64,
    ) -> Result<Vec<ene_learning::Memory>, ene_learning::LearningTechnicalError> {
        Ok(self
            .0
            .iter()
            .map(|recalled| ene_learning::Memory {
                id: recalled.id,
                revision: ene_learning::MemoryRevision::initial(),
                scope: ene_learning::LearningScope::companion(companion),
                content: recalled.content.clone(),
                importance: ene_learning::Importance::clamped(1),
                temporal: ene_learning::TemporalMeaning::Enduring,
                recall_suppressed: false,
                updated_at: WallClockWithTz::now(),
            })
            .collect())
    }

    async fn load_summaries(
        &self,
        _ids: &[ene_learning::SummaryId],
    ) -> Result<Vec<ene_learning::SummaryRecord>, ene_learning::LearningTechnicalError> {
        Err(ene_learning::LearningTechnicalError::StorageUnavailable {
            reason: String::from("assembly fixture is read-only"),
        })
    }
}

#[tokio::test]
async fn assembled_dialogue_prompt_is_scrubbed_again_by_the_credential_boundary() {
    let registry = FixtureRegistry::new(4);
    let store = FixtureStore::for_refs(&registry);
    let scrubber = CredentialScrubber {
        refs: &registry,
        store: &store,
    };
    let companion = CompanionId::from_raw(RawId::new());

    // The mid-assembly credential joins the registry only when the final
    // boundary pass lists refs. Every fragment pass ran before it existed,
    // so the assembled prompt still carries the raw value, and only the
    // final pass can see and redact it.
    registry.expand_on_refs_read(4, 7);
    let history = vec![history_item("context fragment")];
    let recalled = vec![RecalledMemory {
        id: ene_learning::MemoryId::generate(),
        content: format!("note {LATE}"),
    }];

    let input_line = format!("input {ALPHA} tail");
    let proof = assemble_dialogue_input(
        companion,
        input_line.as_str(),
        &FixedHistory(&history),
        &FixedLearning(&recalled),
        &scrubber,
    )
    .await
    .expect("every registered value is removable");
    let proof = proof.prompt();

    assert!(
        !proof.text().contains(LATE),
        "the final assembly pass must redact what fragment passes never saw: {:?}",
        proof.text()
    );
    assert!(
        !proof.text().contains(ALPHA),
        "the final pass keeps earlier redactions: {:?}",
        proof.text()
    );
    assert!(proof.text().contains("[credential]"));
    // The premise stays the oldest preparation pass (4); the mid-assembly
    // registration moved the registry to 7 but must not refresh the proof.
    assert_eq!(proof.credential_set(), CredentialSetRevision::from_u64(4));
    // The armed expansion demonstrably fired during the final pass.
    assert_eq!(registry.current(), CredentialSetRevision::from_u64(7));
}

#[tokio::test]
async fn revision_drift_during_assembly_preserves_the_oldest_premise() {
    let registry = FixtureRegistry::new(2);
    let store = FixtureStore::for_refs(&registry);
    let scrubber = CredentialScrubber {
        refs: &registry,
        store: &store,
    };
    let companion = CompanionId::from_raw(RawId::new());
    let history = vec![history_item("context text")];
    let recalled = Vec::new();

    // The approval lands on the second revision read of this single
    // assembly: the input fragment premise is read at revision 2, the set
    // moves to 9 before the final pass reads it. The proof must keep the
    // oldest premise (2), never refresh to the final 9.
    registry.advance_on_read(2, 9);
    let proof = assemble_dialogue_input(
        companion,
        "input that drifts",
        &FixedHistory(&history),
        &FixedLearning(&recalled),
        &scrubber,
    )
    .await
    .expect("scrubbing succeeds");
    let proof = proof.prompt();

    // The drift demonstrably fired mid-assembly: the registry names 9 now.
    assert_eq!(
        registry.current(),
        CredentialSetRevision::from_u64(9),
        "the armed drift must have fired during this assembly"
    );
    assert_eq!(
        proof.credential_set(),
        CredentialSetRevision::from_u64(2),
        "the fragment premise read at revision 2 must survive the final scrub at 9"
    );
}

#[tokio::test]
async fn the_assembled_read_set_names_exactly_the_memory_and_history_rows_it_consumed() {
    let registry = FixtureRegistry::new(1);
    let store = FixtureStore::for_refs(&registry);
    let scrubber = CredentialScrubber {
        refs: &registry,
        store: &store,
    };
    let companion = CompanionId::from_raw(RawId::new());
    // Two History rows (the caller receives them oldest-first) and two
    // recalled Memories (rank order, as `recall` returned them).
    let older = history_item("the older message");
    let newer = history_item("the newer message");
    let older_id = older.id;
    let newer_id = newer.id;
    let first_memory = ene_learning::MemoryId::generate();
    let second_memory = ene_learning::MemoryId::generate();
    let history = vec![older, newer];
    let recalled = vec![
        RecalledMemory {
            id: first_memory,
            content: String::from("identical memory body"),
        },
        RecalledMemory {
            id: second_memory,
            content: String::from("identical memory body"),
        },
    ];

    let input = assemble_dialogue_input(
        companion,
        "what did we say?",
        &FixedHistory(&history),
        &FixedLearning(&recalled),
        &scrubber,
    )
    .await
    .expect("assembling and scrubbing succeeds");

    // Logical-input order: the Memory section renders before the
    // conversation section, and each section keeps its rendered order.
    assert_eq!(
        input.data_use(),
        &[
            first_memory.as_raw(),
            second_memory.as_raw(),
            older_id,
            newer_id,
        ],
        "the read-set must name exactly the rows the prompt consumed, in input order"
    );
}
