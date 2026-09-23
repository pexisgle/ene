//! Companion identity, history, and undelivered-reporting contracts.
//!
//! [`CompanionId`] names a companion. [`HistoryMessage`] facts are the
//! Host-filtered display record; [`UndeliveredRef`] facts track items that
//! were durably stored but not yet confirmed as presented. An entry only
//! correlates to a canonical [`UndeliveredSource`] fact owned by Task,
//! Action, or History and never copies a body; presentation confirmation
//! arrives as `ene-presentation` observations mapped to a
//! [`PresentationMark`] at the call boundary, and this crate takes no
//! dependency on `ene-presentation` or on any store.
//!
//! Implementor contract (atomic-read rule): the store implementation behind
//! [`HistoryRepository`] reads the current presence generation and the
//! current [`CompanionLifecycle`] in the same short transaction it compares
//! `expected_generation` in, then either inserts or returns an `Ok`-side
//! [`HistoryAppendOutcome`]. Stale expectations return
//! `Ok(HistoryAppendOutcome::StaleExpected { .. })` and stopped or deleted
//! companions return `Ok(HistoryAppendOutcome::HeldByLifecycle { .. })`;
//! [`CompanionTechnicalError`] is reserved for infrastructure failure, never
//! for stale / held outcomes. [`UndeliveredRepository`] follows the same
//! split: parent-durable checks and per-row compare-and-mark are atomic,
//! while stale marks return `Ok(ReportStatusTransition::StaleSource)`.
//! [`UndeliveredRepository::list_unpresented`] is a SELECT-only bounded
//! keyset read: it never mutates status, generations, Task revisions, or
//! report state.
//!
//! Wire mapping (read-only): [`HistoryMessage`] maps to/from
//! `ene_api::v1::round::HistoryItem` plus its companion, message identity,
//! and generation pins at the Host boundary. This crate performs no wire
//! mapping itself.

pub mod dialogue;
use ene_credential::CredentialSetRevision;
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{TaskPurposeRef, TaskRef};

/// Wraps a [`RawId`]; never converted to any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompanionId(RawId);

impl CompanionId {
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

/// Command-scoped idempotency identity for history appends.
///
/// Opaque over [`RawId`] with no `From` conversions: the wire `CommandWireId`
/// maps 1:1 at ingress when the Host parses its UUID text into this domain
/// newtype. The client mints one per send; a transport retry reuses the same
/// command id with a fresh message id. Non-secret correspondence, visible in
/// [`core::fmt::Debug`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub RawId);

/// Canonical client round intent of one command-scoped send (IPC §13.1).
///
/// This is request semantics the Client minted, not anything the Host
/// decided: [`Self::Auto`] names the join-or-mint request,
/// [`Self::New`] names the explicit new-round request, and
/// [`Self::Existing`] names a join of one specific round by the round
/// reference exactly as the Client supplied it. The `Existing` payload
/// stays the wire text the Client sent — never the Host-resolved domain
/// round and never the Host-minted round projection, which are part of
/// the accepted result and never decide replay.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoundIntentMark {
    /// Join the matching open round; mint a fresh one when none matches.
    Auto,
    /// Always mint a fresh round, even when an open round would match.
    New,
    /// Join the round this Client-supplied reference names.
    Existing(String),
}

/// Companion lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompanionLifecycle {
    /// Running and able to accept history appends.
    Running,
    /// Stopped. Appends are held, never applied.
    Stopped,
    /// Deleted. Appends are held, never applied.
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryRole {
    Owner,
    Companion,
}

/// One durable history item: Host-filtered display fact.
///
/// Filtered facts only, never undelivered reporting. Text is redacted from
/// [`core::fmt::Debug`]; refs stay visible.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HistoryMessage {
    pub id: RawId,
    pub companion: CompanionId,
    pub round: RawId,
    pub role: HistoryRole,
    pub text: String,
    pub lang: String,
    /// Wall-clock time with its creation offset, display only.
    pub at: WallClockWithTz,
    pub presence_generation: PresenceGeneration,
    /// Command-scoped idempotency identity, when the caller carries one.
    /// [`None`] marks pre-command callers or an unknown command. The durable
    /// replay key; `local_id` stays as correspondence metadata only.
    pub command_id: Option<CommandId>,
    /// Opaque wire projection of `round`, minted fresh by the caller per
    /// round and unrelated to the domain bytes: the only round string that
    /// ever crosses the wire. [`None`] marks pre-opaque rows.
    pub round_wire: Option<String>,
    /// Canonical client round intent of the command that stored this row,
    /// as [`RoundIntentMark`]. [`None`] marks rows without a command key
    /// (replies) and rows stored before intents were persisted — neither
    /// can prove sameness for replay.
    pub round_intent: Option<RoundIntentMark>,
    /// Client incarnation that sent the item, as `(counter, random)` when
    /// the caller carried one. Part of the request fingerprint together
    /// with text and language. [`None`] must match [`None`]: an epoch-less
    /// key only replays epoch-less.
    pub incarnation: Option<(u64, u64)>,
    /// Client-local correspondence ID for matching an input to its ack.
    /// Correspondence metadata only, no longer the durable key (that is
    /// `command_id`).
    pub local_id: Option<String>,
}

impl core::fmt::Debug for HistoryMessage {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HistoryMessage")
            .field("id", &self.id)
            .field("companion", &self.companion)
            .field("round", &self.round)
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .field("lang", &self.lang)
            .field("at", &self.at)
            .field("presence_generation", &self.presence_generation)
            .field("command_id", &self.command_id)
            .field("round_wire", &self.round_wire)
            .field("round_intent", &self.round_intent)
            .field("incarnation", &self.incarnation)
            .field("local_id", &self.local_id)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AppendHistoryCommand {
    pub companion: CompanionId,
    pub round: RawId,
    pub role: HistoryRole,
    /// Item text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    pub lang: String,
    /// Wall-clock time with its creation offset, display only.
    pub at: WallClockWithTz,
    pub expected_generation: PresenceGeneration,
    /// Consent premise the caller relied on, as an opaque `(id, rev)` pair
    /// that travels together (never a bare revision). [`None`] skips the
    /// consent check; callers that went through admission always pass
    /// [`Some`]. The store compares both inside the append transaction, so
    /// a mid-flight consent move answers [`HistoryAppendOutcome::StaleConsent`]
    /// instead of attributing content across the move.
    pub expected_consent: Option<(String, u64)>,
    /// Credential-set premise the text was scrubbed under. The store
    /// compares it inside the append transaction; content scrubbed before a
    /// credential became registered answers
    /// [`HistoryAppendOutcome::StaleCredentialSet`] instead of landing raw.
    /// [`None`] skips the check (tests and non-content appends).
    pub expected_credential_set: Option<CredentialSetRevision>,
    /// Durable identity of the accepted Owner message this reply answers.
    /// The store compares it inside the append transaction against the
    /// latest accepted Owner message for the companion: a newer Owner
    /// input answers [`HistoryAppendOutcome::StaleOwnerInput`] instead of
    /// adopting a superseded reply. [`None`] skips the check (owner
    /// appends, which establish recency rather than answer it).
    pub expected_owner_message: Option<RawId>,
    /// Command-scoped idempotency identity, when the caller carries one.
    /// [`None`] stores NULL (no replay key). A retry reuses the same command
    /// id with a fresh message id; `local_id` stays as correspondence
    /// metadata only.
    pub command_id: Option<CommandId>,
    /// Opaque wire projection of `round`, minted fresh by the caller per
    /// round and unrelated to the domain bytes. The store persists it so
    /// replay acks and timeline views echo it back instead of rendering the
    /// domain identity.
    pub round_wire: Option<String>,
    /// Canonical client round intent of this request, as
    /// [`RoundIntentMark`]. Required whenever `command_id` is [`Some`]: the
    /// intent is request semantics, so a keyed append without one stores an
    /// unprovable row that every later retry declines. [`None`] is for
    /// keyless appends (replies).
    pub round_intent: Option<RoundIntentMark>,
    /// Client incarnation that sends the item, as `(counter, random)`.
    /// Part of the request fingerprint; [`None`] must match [`None`], so an
    /// epoch-less key only replays epoch-less.
    pub incarnation: Option<(u64, u64)>,
    /// Client-local correspondence ID for matching an input to its ack, if
    /// the caller carries one. [`None`] stores NULL. Correspondence metadata
    /// only, no longer the durable key (that is `command_id`).
    pub local_id: Option<String>,
}

impl core::fmt::Debug for AppendHistoryCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AppendHistoryCommand")
            .field("companion", &self.companion)
            .field("round", &self.round)
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .field("lang", &self.lang)
            .field("at", &self.at)
            .field("expected_generation", &self.expected_generation)
            .field("expected_consent", &self.expected_consent)
            .field("expected_credential_set", &self.expected_credential_set)
            .field("expected_owner_message", &self.expected_owner_message)
            .field("command_id", &self.command_id)
            .field("round_wire", &self.round_wire)
            .field("round_intent", &self.round_intent)
            .field("incarnation", &self.incarnation)
            .field("local_id", &self.local_id)
            .finish()
    }
}

impl AppendHistoryCommand {
    /// Builds the [`RequestFingerprint`] this append would store.
    ///
    /// [`None`] when the command carries no replay key (`command_id` is
    /// [`None`]): there is nothing to compare. A keyed command without a
    /// round intent has no fingerprint either — the store treats that
    /// against a stored row as unprovable sameness and declines it.
    #[must_use]
    pub fn request_fingerprint(&self) -> Option<RequestFingerprint> {
        self.command_id.as_ref()?;
        Some(RequestFingerprint {
            role: self.role,
            text: self.text.clone(),
            lang: self.lang.clone(),
            incarnation: self.incarnation,
            round_intent: self.round_intent.clone()?,
        })
    }
}

impl HistoryMessage {
    /// Reads the stored [`RequestFingerprint`] of this row, when provable.
    ///
    /// [`None`] marks rows without a command key (replies) and rows stored
    /// without a round intent (pre-mark rows): their sameness cannot be
    /// proven, so replay callers fail closed instead of exact-replaying.
    #[must_use]
    pub fn request_fingerprint(&self) -> Option<RequestFingerprint> {
        self.command_id.as_ref()?;
        Some(RequestFingerprint {
            role: self.role,
            text: self.text.clone(),
            lang: self.lang.clone(),
            incarnation: self.incarnation,
            round_intent: self.round_intent.clone()?,
        })
    }
}

/// Outcome of a history append attempt.
///
/// An `Ok`-side domain outcome, never an error. Stale and held outcomes are
/// returned as `Ok`, never retried automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryAppendOutcome {
    /// Committed as this message identity: the caller owns this row and
    /// continues with inference, reply, and streaming for it.
    CommittedAs { message: RawId },
    /// Already committed under the same idempotency key by a concurrent
    /// attempt: the caller must NOT re-run inference or re-append. It
    /// answers the original acceptance (round below) and stops, so a
    /// transport retry racing the original can neither duplicate effects
    /// nor steal the round.
    AlreadyCommittedAs { message: RawId, round: RawId },
    /// The `expected_generation` was not current.
    StaleExpected {
        /// Current generation the caller should observe next time.
        current: PresenceGeneration,
    },
    /// The expected consent moved underneath this append. The generation
    /// premise held, but the consent premise did not, so the content must
    /// not be attributed to the new consent without a fresh check. Carries
    /// no payload: the caller reloads and answers `consent-stale`.
    StaleConsent,
    /// The credential set moved past the scrub premise. The text may carry a
    /// newly registered credential value and must not be committed; the
    /// caller re-scrubs and retries (owner input) or interrupts (reply).
    StaleCredentialSet,
    /// A newer accepted Owner input superseded this turn's owner before the
    /// reply append ran. Generation, consent, and credential premises may
    /// all still hold: the reply would answer input the owner already moved
    /// past, so it must not be adopted. Distinct from
    /// [`HistoryAppendOutcome::StaleExpected`], which is presence-generation
    /// staleness only. The caller interrupts the stream; the superseding
    /// input's own turn proceeds normally.
    StaleOwnerInput,
    /// A reused command key arrived with a different request than the
    /// stored row: the stored [`RequestFingerprint`] (role, body, language,
    /// sending incarnation, and canonical round intent) did not match — or
    /// could not be reconstructed, which fails closed the same way. The
    /// accepted result (round identity and its wire projection) never
    /// decides this: a retry whose Host re-intaked it into a newer round
    /// replays the stored accept verbatim instead of conflicting. Declined
    /// without side effects: no new row, no undelivered registration, no
    /// inference. Carries no payload (the key itself is the caller's
    /// correlation): the Host answers a typed wire rejection, never an
    /// intake outcome, and never retries automatically. Resending the
    /// original request replays as
    /// [`HistoryAppendOutcome::AlreadyCommittedAs`] instead.
    CommandConflict,
    /// Held by the companion lifecycle.
    HeldByLifecycle { lifecycle: CompanionLifecycle },
    /// A canonical current erasure condition covers this body or the accepted
    /// Owner input it derives from (lifecycle §7/§11). Nothing was written:
    /// no History row, no undelivered registration, no round.
    ///
    /// This is the delayed-arrival boundary: a body produced before (or
    /// during) a deletion operation is refused here instead of being
    /// re-saved, whether it arrives as a fresh append, a transport retry, or
    /// a reconnecting Client's local-copy replay. The outcome carries no
    /// payload — not even which condition matched — so a rejection path can
    /// never leak the target body or its correlation.
    ///
    /// It is a domain refusal, distinct from `StaleCredentialSet` (the
    /// scrub premise moved) and `CommandConflict` (the replay fingerprint
    /// disagreed): the body must not be re-created under a current deletion,
    /// while a credential or command staleness would be answered differently.
    HeldForErasure,
}

/// The immutable request semantics of one command-scoped history append:
/// role, body text, language, sending incarnation, and the canonical
/// client round intent.
///
/// This is the whole comparison for command replay, in one type used by
/// every caller — the store compares it in-transaction and the Host's
/// early replay path compares the very same value, so there is exactly one
/// fingerprint definition and one equality. What stays out matters as
/// much: the accepted result (domain round, Host-minted round projection),
/// every transport or correspondence ID (`message_id`, `local_id`), and
/// the generation premise (enforced separately) never decide replay.
///
/// Body text is redacted from [`core::fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct RequestFingerprint {
    pub role: HistoryRole,
    pub text: String,
    pub lang: String,
    /// Sending incarnation. [`None`] must match [`None`].
    pub incarnation: Option<(u64, u64)>,
    pub round_intent: RoundIntentMark,
}

impl core::fmt::Debug for RequestFingerprint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RequestFingerprint")
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .field("lang", &self.lang)
            .field("incarnation", &self.incarnation)
            .field("round_intent", &self.round_intent)
            .finish()
    }
}

/// Durable identity of one undelivered item, minted by the companion owner.
///
/// Opaque over [`RawId`]. The store's non-reused insertion sequence is a
/// storage order only and is never this identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredId(RawId);

impl UndeliveredId {
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

/// Closed-world Action certainty at the undelivered boundary.
///
/// The Action owner's vocabulary projected as a plain value, never an
/// imported `ene-action` newtype: the SQL source phase of an
/// `action_attempt` source is exactly [`Self::as_str`], and a later certainty
/// is a new source key, so an old `Unknown` entry is never shown as the
/// current certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCertaintyWire {
    ConfirmedSuccess,
    ConfirmedFailure,
    Unknown,
}

impl ActionCertaintyWire {
    /// Stable source-phase name, closed world.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedSuccess => "confirmed_success",
            Self::ConfirmedFailure => "confirmed_failure",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "confirmed_success" => Some(Self::ConfirmedSuccess),
            "confirmed_failure" => Some(Self::ConfirmedFailure),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Closed-world terminal Task transition at the undelivered boundary.
///
/// `Completed` is deliberately absent: completion is described by
/// [`TaskFact::ResultAdopted`], so one transition never registers two
/// notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalKindWire {
    Failed,
    Cancelled,
}

impl TerminalKindWire {
    /// Stable source-phase name, closed world.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// One Task-owned fact an undelivered item can report on (CI §5.2).
///
/// Every variant is a correlation to a canonical fact owned by the Task or
/// Action repository; it is never a state copy, a body, or an execution
/// instruction. A later fact is a new source key and therefore a new
/// notification, and the source's body always reads the current owner row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskFact {
    /// One durable revision snapshot (AU2 creation or AU4/AU17 forward).
    TaskRevision { task: RawId, revision: u64 },
    /// One accepted delegation (AU3/AU17).
    Delegation(RawId),
    /// One Action attempt at one certainty (AU5 insert or the certainty CAS).
    ActionAttempt {
        attempt: RawId,
        certainty: ActionCertaintyWire,
    },
    /// One final result arrival and execution seal (AU15a).
    ResultRecorded(RawId),
    /// One result adoption stamp (AU15b `adopted_revision`).
    ResultAdopted(RawId),
    /// One terminal Task transition. `Completed` is [`Self::ResultAdopted`].
    Terminal {
        task: RawId,
        progress: TerminalKindWire,
    },
}

/// The closed sum of canonical sources an undelivered item correlates to.
///
/// Task-derived facts carry no round: the notification is registered by the
/// Task or Action commit and reports a fact, not a conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UndeliveredSource {
    /// A fact under one Task record.
    TaskRecord { task: RawId, fact: TaskFact },
    /// One stored conversation message (History owner).
    HistoryMessage(RawId),
    /// One stored activity record (companion owner).
    ActivityRecord(RawId),
}

/// Undelivered tracking fact: a durably stored item not yet confirmed.
///
/// The entry is a correlation plus reporting status only. Bodies, current
/// progress, delegation, and the latest result are never copied here; report
/// composition reads them from the source owner. `round` /
/// `presence_generation` are `None` for task-derived notifications, which
/// have no originating conversation round; a fabricated round is never
/// invented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredRef {
    pub id: UndeliveredId,
    pub companion: CompanionId,
    pub source: UndeliveredSource,
    pub status: ReportStatus,
    /// When the source fact committed.
    pub created_at: WallClockWithTz,
    /// Originating round, when the source came from a conversation turn.
    pub round: Option<RawId>,
    /// Presence generation of that round, when one existed.
    pub presence_generation: Option<PresenceGeneration>,
}

/// Reporting status of one undelivered item (CI §5.2).
///
/// [`Self::PresentationUnknown`] is re-presentable, never sticky terminal: a
/// next receipt or explicit re-display reads it again, and only
/// [`Self::Presented`] is absorbing for the source fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatus {
    /// Presentation has not started, or a current receipt confirmed the item
    /// was not presented.
    Pending,
    /// Presentation started (or its outcome is unconfirmed). Listed again by
    /// [`UndeliveredRepository::list_unpresented`].
    PresentationUnknown,
    /// The item's presentation was confirmed. Absorbing.
    Presented,
}

/// An `Ok`-side domain outcome, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatusTransition {
    /// A confirmed presentation moved the row to [`ReportStatus::Presented`].
    PendingToPresented,
    /// Presentation started: the row is now
    /// [`ReportStatus::PresentationUnknown`].
    MarkedPresentationUnknown,
    /// The row was already [`ReportStatus::Presented`]; nothing was written.
    AlreadyPresented,
    /// A current receipt confirmed the row was not presented:
    /// [`ReportStatus::PresentationUnknown`] returned to
    /// [`ReportStatus::Pending`].
    FailedToPending,
    /// The `expected` status was not current, or the identity is unknown.
    StaleSource,
    /// A canonical current erasure condition covers the row's source body
    /// (lifecycle §7/§11). No status was written: neither a presentation
    /// start nor a confirmation may claim an item under an active deletion.
    ///
    /// Distinct from [`Self::StaleSource`] (the status premise moved) and
    /// [`Self::AlreadyPresented`] (an absorbing duplicate): the item itself
    /// is under deletion, so the caller must not report it as presented and
    /// must not retry the same transition while the condition holds.
    HeldForErasure,
}

/// One bounded page of unpresented entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeliveredPage {
    /// Entries in insertion order.
    pub entries: Vec<UndeliveredRef>,
    /// Continuation inside the pass; [`None`] means the pass reached its
    /// captured upper bound.
    pub next: Option<UndeliveredCursor>,
    /// Insertion bound this page read against. A drained pass leaves the
    /// caller's scan lower bound at this value for the next pass.
    pub pass_upper_bound: u64,
}

/// Keyset cursor over the undelivered insertion sequence (PR §4.6).
///
/// The sequence is the store's non-reused insertion key, never a Task
/// revision or a presence generation. A pass captures the sequence in force
/// when it begins ([`Self::begin`]); rows registered while the pass runs are
/// returned by the next pass instead of shifting the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredCursor {
    after_seq: u64,
    pass_upper_bound: u64,
}

impl UndeliveredCursor {
    /// Continues a pass strictly after `after_seq`, up to the bound captured
    /// when the pass began.
    #[must_use]
    pub const fn begin(after_seq: u64, pass_upper_bound: u64) -> Self {
        Self {
            after_seq,
            pass_upper_bound,
        }
    }

    /// The insertion sequence this cursor has already scanned.
    #[must_use]
    pub const fn after_seq(self) -> u64 {
        self.after_seq
    }

    /// The bound captured when the pass began.
    #[must_use]
    pub const fn pass_upper_bound(self) -> u64 {
        self.pass_upper_bound
    }
}

/// Maximum entries one [`UndeliveredRepository::list_unpresented`] page
/// returns (IPC §13.3: a page carries at most 50 items).
pub const UNDELIVERED_PAGE_MAX: u32 = 50;

/// Presentation observation mapped to the undelivered boundary.
///
/// `presented` is the observation: `true` confirms the item was on screen;
/// `false` is the presentation start when compared against
/// [`ReportStatus::Pending`] and a not-presented receipt when compared
/// against [`ReportStatus::PresentationUnknown`]. Sending alone never marks
/// an entry; receipt currentness is the caller's premise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationMark {
    pub round: RawId,
    pub presented: bool,
}

/// Infrastructure failure for companion and history operations.
///
/// Stale / held outcomes are [`HistoryAppendOutcome`], never this error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompanionTechnicalError {
    #[error("companion storage unavailable: {reason}")]
    StorageUnavailable {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

/// Infrastructure failure for undelivered operations.
///
/// Stale marks are [`ReportStatusTransition::StaleSource`], never this error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UndeliveredTechnicalError {
    #[error("undelivered storage unavailable: {reason}")]
    StorageUnavailable {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CompanionRepository {
    /// Ensures the running companion exists, creating it when absent.
    ///
    /// The single-companion shape keeps intake and history pinned to one
    /// lifecycle; multi-companion selection arrives with its owner.
    async fn ensure_running_companion(&self) -> Result<CompanionId, CompanionTechnicalError>;

    /// Loads the lifecycle for one companion.
    ///
    /// Returns [`None`] when the companion is unknown. Absence is reported,
    /// never defaulted.
    async fn load_lifecycle(
        &self,
        companion: CompanionId,
    ) -> Result<Option<CompanionLifecycle>, CompanionTechnicalError>;
}

/// History append and timeline contract.
///
/// The implementor reads the current presence generation and the current
/// [`CompanionLifecycle`] in the same short transaction it compares
/// `expected_generation` in, then either inserts or returns an `Ok`-side
/// [`HistoryAppendOutcome`]. The atomic section never performs inference,
/// presentation, or other long work.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait HistoryRepository {
    /// Appends one message under a generation expectation.
    ///
    /// Stale expectations return
    /// `Ok(HistoryAppendOutcome::StaleExpected { .. })`; stopped or deleted
    /// companions return `Ok(HistoryAppendOutcome::HeldByLifecycle { .. })`.
    async fn append_message(
        &self,
        cmd: AppendHistoryCommand,
    ) -> Result<HistoryAppendOutcome, CompanionTechnicalError>;

    /// Appends one companion reply and, when `register_unpresented` holds,
    /// registers an undelivered entry for it in the same atomic section.
    ///
    /// Conversation-sourced undelivered registration shares the history
    /// append atom (AU1a); Task- and Action-sourced registration shares the
    /// parent fact's own commit instead (AU1b). The returned [`Option`]
    /// carries the registered [`UndeliveredRef`] when registration happened.
    ///
    /// `inference_claim` is the durable provider claim this reply was
    /// produced under, when the caller obtained one. The implementor compares
    /// it inside the same transaction against the canonical deletion
    /// correspondence: a claim a deletion admission already associated with
    /// an interval is refused with [`HistoryAppendOutcome::HeldForErasure`]
    /// even after the operation completed and no current condition is
    /// readable (lifecycle §11 R2). [`None`] skips the check (non-provider
    /// appends and direct test fixtures).
    async fn append_reply_with_undelivered(
        &self,
        cmd: AppendHistoryCommand,
        register_unpresented: bool,
        inference_claim: Option<RawId>,
    ) -> Result<(HistoryAppendOutcome, Option<UndeliveredRef>), CompanionTechnicalError>;

    /// Loads timeline items for one companion, oldest first.
    ///
    /// `since` bounds items at or after that instant when set; `round`
    /// restricts to one domain round when set; `limit` caps the number
    /// returned. All three are applied by the storage query, so the bound is
    /// on the rows read and decoded, not only on the returned vector.
    /// Wall-clock bounds are display selection only, never currentness
    /// evidence.
    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        round: Option<RawId>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError>;

    /// Loads the newest items for one companion, oldest first within the
    /// returned window, capped at `limit`.
    ///
    /// This is the bounded recent-context query: callers that need the
    /// conversation near the present (dialogue context, Experience source)
    /// must not read the whole timeline to find it.
    ///
    /// Items under a current deletion condition are withheld from the
    /// returned window (the implementor compares the canonical premise inside
    /// the read), so a recent-context read never hands a covered body to a
    /// consumer that would put it into a provider input. An unreadable
    /// current condition withholds the whole window.
    async fn load_recent_timeline(
        &self,
        companion: CompanionId,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError>;

    /// Looks up one previously accepted message by command id.
    ///
    /// The replay path calls this before appending: when a record exists the
    /// caller returns the original acceptance without re-appending, so
    /// retries of the same command stay idempotent. Stream outcomes are not
    /// replayed through this lookup; a caller that needs missed stream items
    /// recovers via `HistoryRequest`.
    async fn lookup_command(
        &self,
        companion: CompanionId,
        command: &CommandId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError>;

    /// Loads one history item by its `message_id` primary key.
    ///
    /// This is the single-message bounded read: exactly the addressed row is
    /// read, never a timeline load, a recent window, or a command lookup.
    /// [`None`] reports absence (the row does not exist); a malformed durable
    /// row is a technical error, never a composed substitute. The returned
    /// row is decoded by the same decoder every other History read uses, so
    /// there is no second decoding contract.
    async fn load_message(
        &self,
        message: RawId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError>;
}

/// Undelivered registration and reporting contract.
///
/// Registration happens only when the parent fact is durable, inside the
/// parent's transaction, so this trait exposes no standalone registration
/// API. Listing and status changes are SELECT / compare commits only: reads
/// never mutate status, generations, Task revisions, or report state.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UndeliveredRepository {
    /// Compares `expected` against the current [`ReportStatus`] and, on
    /// match, applies `mark` in one atomic per-row compare.
    ///
    /// `mark.presented = true` is a confirmed presentation and writes
    /// [`ReportStatus::Presented`]; a duplicate ACK on an already presented
    /// row writes nothing and answers
    /// [`ReportStatusTransition::AlreadyPresented`], because `Presented`
    /// absorbs every later mark. `mark.presented = false` against
    /// `expected = Pending` is the presentation start and writes
    /// [`ReportStatus::PresentationUnknown`]; against
    /// `expected = PresentationUnknown` it is a current receipt that
    /// confirmed the item was not presented and returns the row to
    /// [`ReportStatus::Pending`] ([`ReportStatusTransition::FailedToPending`]).
    /// Receipt currentness (connection, Round, presence generation, selected
    /// id set) is validated by the caller before this call; this compare
    /// judges only row status and the `expected` premise.
    ///
    /// Mismatch returns `Ok(ReportStatusTransition::StaleSource)`, never
    /// `Err`.
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;

    /// Captures the insertion sequence in force, for the next pass.
    ///
    /// `0` means no entry exists yet. This is a read: it never advances a
    /// generation, writes a marker, or reconciles anything.
    async fn undelivered_pass_bound(&self) -> Result<u64, UndeliveredTechnicalError>;

    /// Lists one bounded page of `Pending` / `PresentationUnknown` entries,
    /// oldest insertion first, scoped to `companion`.
    ///
    /// `cursor: None` begins a pass at the head and captures the current
    /// insertion bound; `Some` continues (or resumes) a pass, and the cursor
    /// keeps rows registered during the pass out of it. `limit` is the page
    /// bound and is applied by the storage query (clamped to `1..=50`), so
    /// the bound is on the rows read, not only on the returned vector. The
    /// read changes nothing: no status, generation, or Task revision is
    /// written, and no reconciliation runs.
    async fn list_unpresented(
        &self,
        companion: CompanionId,
        cursor: Option<UndeliveredCursor>,
        limit: u32,
    ) -> Result<UndeliveredPage, UndeliveredTechnicalError>;

    /// Resolves exact undelivered identities for `companion`, in the
    /// requested order, with the stored report status on each row.
    ///
    /// This is the exact-identity read a presentation receipt uses to
    /// rehydrate its own selection: head position, later arrivals, and the
    /// number of other rows never affect which ids resolve. An id that is
    /// missing or belongs to another companion is omitted, so the caller
    /// compares lengths to detect an unrehydratable selection. `ids` is
    /// bounded by the page bound ([`UNDELIVERED_PAGE_MAX`]; extra ids are
    /// ignored). The read changes nothing.
    async fn load_undelivered_by_ids(
        &self,
        companion: CompanionId,
        ids: &[UndeliveredId],
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError>;
}

/// Durable identity of one first-party management activity record, minted by
/// the companion owner.
///
/// Opaque over [`RawId`]. The store's row order is a storage order only and
/// is never this identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActivityId(RawId);

impl ActivityId {
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

/// One recorded first-party Owner management input (H-A.1 resume source).
///
/// The body is the Owner's resume instruction text and stays canonical in
/// this record: Task state carries only the reference, never a copy. Text
/// is redacted from [`core::fmt::Debug`]; refs stay visible.
#[derive(Clone, PartialEq, Eq)]
pub struct ManagementActivity {
    pub id: ActivityId,
    pub companion: CompanionId,
    /// The explicitly selected Task and revision the instruction continues.
    pub task: TaskRef,
    /// The purpose identity in force at selection.
    pub purpose: TaskPurposeRef,
    /// The resume instruction body.
    pub body: String,
    /// When the record was accepted.
    pub created_at: WallClockWithTz,
}

impl core::fmt::Debug for ManagementActivity {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ManagementActivity")
            .field("id", &self.id)
            .field("companion", &self.companion)
            .field("task", &self.task)
            .field("purpose", &self.purpose)
            .field("body", &"[redacted]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Records one resume instruction activity from the first-party management
/// inlet.
///
/// `command` is the idempotency key of the recording epoch (the management
/// intent id): the same key returns the same activity without recording
/// again, and a retry never mints a second record.
#[derive(Clone, PartialEq, Eq)]
pub struct RecordResumeActivityCommand {
    pub companion: CompanionId,
    pub task: TaskRef,
    pub purpose: TaskPurposeRef,
    /// The Owner's resume instruction body. Redacted from [`core::fmt::Debug`].
    pub body: String,
    /// Idempotency key of the recording epoch.
    pub command: RawId,
}

impl core::fmt::Debug for RecordResumeActivityCommand {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RecordResumeActivityCommand")
            .field("companion", &self.companion)
            .field("task", &self.task)
            .field("purpose", &self.purpose)
            .field("body", &"[redacted]")
            .field("command", &self.command)
            .finish()
    }
}

/// Outcome of one resume-instruction activity record.
///
/// A canonical current erasure condition covering the instruction body is a
/// domain hold, distinct from a store failure: the activity (and the resume it
/// feeds) is refused rather than re-saving the covered body. A completed
/// operation is not a current condition, so a fresh resume after completion
/// records normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeActivityOutcome {
    /// The activity is durable under its command key.
    Recorded(ActivityId),
    /// The instruction body is under an active deletion; nothing was written.
    HeldForErasure,
}

/// First-party management activity contract (owner: companion).
///
/// The single-record bounded read ([`Self::load_activity`]) is what the
/// Task-owned instruction-source port resolves an `OwnerManagement` origin
/// against; timeline loads and command lookups are not a substitute.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ActivityRepository {
    /// Records one resume instruction activity, idempotent by `command`.
    ///
    /// The same command key returns the recorded activity identity without
    /// writing again; a key that already names different content fails
    /// closed instead of being reinterpreted.
    async fn record_resume_activity(
        &self,
        cmd: RecordResumeActivityCommand,
    ) -> Result<ResumeActivityOutcome, CompanionTechnicalError>;

    /// Loads one activity record by its primary key.
    ///
    /// [`None`] reports absence; a malformed durable row is a technical
    /// error, never a composed substitute.
    async fn load_activity(
        &self,
        activity: ActivityId,
    ) -> Result<Option<ManagementActivity>, CompanionTechnicalError>;
}
