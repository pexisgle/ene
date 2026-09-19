//! Final Task result arrival, execution seal, and adoption (AU15a / AU15b).
//!
//! A final Task result is recorded at an explicit finalization boundary (the
//! Task Agent execution submits a final answer) before it is visible to any
//! caller: the `task_result` row itself is the execution seal, so one
//! delegation has at most one final result and no separate seal flag exists.
//! Provider output from a single inference turn is **not** a final result; a
//! tool loop may still turn it into an Action request, so the finalization
//! decision stays an explicit caller boundary.
//!
//! Adoption is separate from arrival. The `attempt_refs` a caller supplies are
//! a claim, never the authority: the Task owner enumerates the authoritative
//! Action attempt set from the delegation (execution lifetime), requires an
//! exact match (missing, extra, and duplicate refs are fail-closed technical
//! errors), reads the Action owner's certainty without ever changing it, and
//! additionally requires the Task-wide completion barrier (no `Unknown`
//! attempt anywhere under the same Task) inside the same short transaction as
//! the completion CAS.

use ene_credential::{CredentialSetRevision, ScrubbedText};
use ene_primitive::{RawId, WallClockWithTz};

use crate::agent::TaskAgentOutput;
use crate::delegation::DelegationId;
use crate::task::{TaskId, TaskRef, TaskRevision};

/// Durable identity of one final Task result body.
///
/// The Task owner's orchestration mints it at the explicit finalization
/// boundary; the repository never re-allocates it. One delegation has at most
/// one final result, and retrying the same identity is idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskResultId(RawId);

impl TaskResultId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

/// One keyset position in the sealed-but-unadopted candidate set.
///
/// The pair is a total order over stored `task_result` rows (the stored
/// `recorded_at` text and the result identity) used only to page a bounded
/// reconciliation sweep. It borrows no chronological authority: the wall
/// clock can move backwards and offsets can differ, so the order is a
/// storage-total order for traversal, never currentness evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnadoptedResultCursor {
    pub recorded_at: WallClockWithTz,
    pub result: TaskResultId,
}

/// A final Task result body bound to the credential-set revision it was
/// scrubbed under (AU15a currentness premise).
///
/// The body enters only through a credential-owned [`ScrubbedText`] proof:
/// the fields are private and [`Self::from_scrubbed`] consumes the proof, so
/// no caller — including an external crate or a test fake — can pair
/// arbitrary text with a revision or replace either half afterwards. The
/// durable arrival commit compares [`Self::credential_set`] with the current
/// set inside the same short transaction as the insert and refuses a
/// mismatch as [`TaskResultArrivalOutcome::StaleCredentialSet`], because a
/// scrub reads the revision before the values it covers.
///
/// A forged premise does not compile:
///
/// ```compile_fail
/// use ene_credential::CredentialSetRevision;
/// use ene_task::TaskResultScrubPremise;
/// let forged = TaskResultScrubPremise {
///     body: String::from("unscrubbed"),
///     credential_set: CredentialSetRevision::initial(),
/// };
/// ```
///
/// Neither half can be replaced on an existing premise:
///
/// ```compile_fail
/// use ene_task::TaskResultScrubPremise;
/// fn replace(premise: &mut TaskResultScrubPremise) {
///     premise.body = String::from("unscrubbed");
/// }
/// ```
///
/// A fake scrubber cannot mint the proof the constructor requires:
///
/// ```compile_fail
/// use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
/// use ene_task::TaskResultScrubPremise;
/// struct Fake;
/// impl SecretScrubber for Fake {
///     async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
///         Ok(ScrubbedText {
///             text: text.to_owned(),
///             credential_set: CredentialSetRevision::initial(),
///         })
///     }
/// }
/// async fn mint(fake: &Fake) -> TaskResultScrubPremise {
///     TaskResultScrubPremise::from_scrubbed(fake.scrub("body").await.unwrap())
/// }
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct TaskResultScrubPremise {
    /// Scrubbed copy; the raw input is no longer referenced after this.
    body: String,
    /// Credential-set revision the scrub was produced under.
    credential_set: CredentialSetRevision,
}

impl TaskResultScrubPremise {
    /// Binds a credential-owned scrub proof to the Task arrival premise.
    ///
    /// The proof is consumed, so the premise's body is exactly the text the
    /// credential boundary proved and the revision is the one read before the
    /// values it covers.
    #[must_use]
    pub fn from_scrubbed(proof: ScrubbedText) -> Self {
        let credential_set = proof.credential_set();
        Self {
            body: proof.into_text(),
            credential_set,
        }
    }

    /// The scrubbed result body.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The credential-set revision the body was scrubbed under.
    #[must_use]
    pub fn credential_set(&self) -> CredentialSetRevision {
        self.credential_set
    }
}

/// Diagnostics never carry the body: only the non-secret revision is shown.
impl core::fmt::Debug for TaskResultScrubPremise {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskResultScrubPremise")
            .field("body", &"[redacted]")
            .field("credential_set", &self.credential_set)
            .finish()
    }
}

/// The durable premises of one final result arrival (AU15a).
///
/// The relied Task revision is the delegation row's, never repeated here. The
/// arrival is recorded before any adoption attempt and before the final
/// result is visible; it judges nothing about certainty, terminal state, or
/// completion. The credential-set revision is not judged here either: the
/// durable commit compares [`TaskAgentResultArrival::body`]'s premise inside
/// its own transaction and answers
/// [`TaskResultArrivalOutcome::StaleCredentialSet`] on a mismatch.
///
/// [`core::fmt::Debug`] redacts the body through [`TaskResultScrubPremise`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentResultArrival {
    pub delegation: DelegationId,
    pub result: TaskResultId,
    pub body: TaskResultScrubPremise,
}

/// The Task owner's answer to one final result arrival (AU15a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultArrivalOutcome {
    /// The result committed (or an idempotent same-identity retry replayed)
    /// under the body's still-current credential-set premise.
    Recorded(TaskResultRecord),
    /// The credential set advanced past the body's scrub premise: the commit
    /// wrote nothing and no result row exists. The caller must re-scrub the
    /// original answer under the reported current revision and arrive again
    /// with a fresh premise; the stale body is never retried as it is.
    StaleCredentialSet {
        /// The durable current revision read inside the commit transaction.
        current: CredentialSetRevision,
    },
}

/// A caller's claim about which Action attempts a result relied on.
///
/// The claim is comparison material only: the repository enumerates the
/// authoritative set from the result's delegation and requires an exact
/// set match, so the claim can neither narrow nor widen the durable set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResultAdoptionClaim {
    pub result: TaskResultId,
    pub attempt_refs: Vec<RawId>,
}

/// One final result as durably recorded.
///
/// `task` is the relied Task revision resolved from the delegation row,
/// `attempt_refs` is exactly the result-local verified correlation stamped in
/// `task_result_attempt` (never the Task-wide barrier attempts), and
/// `adopted_revision` is `None` until the adoption commit succeeds.
///
/// [`core::fmt::Debug`] redacts the body through [`TaskAgentOutput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResultRecord {
    pub result: TaskResultId,
    pub task: TaskRef,
    pub delegation: DelegationId,
    pub body: TaskAgentOutput,
    pub attempt_refs: Vec<RawId>,
    pub adopted_revision: Option<TaskRevision>,
    pub recorded_at: WallClockWithTz,
}

/// The Task owner's domain result of one adoption attempt (AU15b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResultAcceptance {
    /// The result matched the current revision and purpose identity, its
    /// authoritative set matched the claim, every relied attempt was
    /// `ConfirmedSuccess`, the Task-wide completion barrier found no
    /// `Unknown`, and the completion CAS committed in the same short
    /// transaction.
    AdoptedAsCompletion(TaskRef),
    /// The current revision moved, the Task is terminal, or a later cancel
    /// marker applies; the result stays durable against its original relied
    /// revision and the current Task is unchanged.
    RecordedToOriginalOnly,
    /// The claim matched the authoritative set, but completion is withheld:
    /// `attempts` are the blockers (relied attempts that are not
    /// `ConfirmedSuccess`, union the Task-wide `Unknown`), deduplicated.
    /// The result-local correlation is recorded; the Task is unchanged and
    /// the same result may be re-evaluated after Action-side evidence
    /// settles.
    WithheldByEffectFacts { attempts: Vec<RawId> },
    /// No result row exists for the identity; nothing was written.
    MissingResult { result: TaskResultId },
    /// The result's delegation has no durable correspondence; nothing was
    /// written.
    MissingDelegation { delegation: DelegationId },
    /// The result's Task has no durable state; nothing was written.
    MissingTask { task: TaskId },
}

#[cfg(test)]
mod result_tests {
    use super::{TaskResultArrivalOutcome, TaskResultScrubPremise};
    use ene_credential::{
        CredentialRef, CredentialRefRepository, CredentialScrubber, CredentialSetRepository,
        CredentialSetRevision, CredentialTechnicalError, MemoryCredentialStore,
        SecretScrubber as _,
    };

    /// A readable registry with no refs, pinned at one revision.
    struct EmptyRegistry(CredentialSetRevision);

    impl CredentialRefRepository for EmptyRegistry {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            Ok(Vec::new())
        }
    }

    impl CredentialSetRepository for EmptyRegistry {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn current_set_revision(
            &self,
        ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
            Ok(self.0)
        }
    }

    async fn scrubbed(revision: CredentialSetRevision, text: &str) -> TaskResultScrubPremise {
        let registry = EmptyRegistry(revision);
        let values = MemoryCredentialStore::new();
        TaskResultScrubPremise::from_scrubbed(
            CredentialScrubber {
                refs: &registry,
                store: &values,
            }
            .scrub(text)
            .await
            .expect("the empty fixture registry is readable"),
        )
    }

    #[tokio::test]
    async fn the_premise_binds_the_scrubbed_body_to_its_revision() {
        let premise = scrubbed(CredentialSetRevision::from_u64(7), "the final report").await;
        assert_eq!(premise.body(), "the final report");
        assert_eq!(premise.credential_set(), CredentialSetRevision::from_u64(7));
        let rendered = format!("{premise:?}");
        assert!(
            !rendered.contains("the final report"),
            "premise Debug redacts the body: {rendered}"
        );
        assert!(rendered.contains("credential_set"), "{rendered}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_stale_outcome_names_only_the_revision() {
        let outcome = TaskResultArrivalOutcome::StaleCredentialSet {
            current: CredentialSetRevision::from_u64(11),
        };
        let rendered = format!("{outcome:?}");
        assert!(rendered.contains("StaleCredentialSet"), "{rendered}");
        assert!(rendered.contains("11"), "{rendered}");
    }
}
