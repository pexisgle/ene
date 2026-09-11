//! Companion identity, history, and undelivered-reporting contracts.
//!
//! [`CompanionId`] names a companion. [`HistoryMessage`] facts are the
//! Host-filtered display record; [`UndeliveredRef`] facts track items that
//! were durably stored but not yet confirmed as presented. Presentation
//! confirmation arrives as `ene-presentation` observations mapped to a
//! [`PresentationMark`] at the call boundary; this crate takes no dependency
//! on `ene-presentation` or on any store.
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
//!
//! Wire mapping (read-only): [`HistoryMessage`] maps to/from
//! `ene_api::v1::round::HistoryItem` plus its companion, message identity,
//! and generation pins at the Host boundary. This crate performs no wire
//! mapping itself.

pub mod dialogue;
use ene_credential::CredentialSetRevision;
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};

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

/// Undelivered tracking fact: a durably stored item not yet confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredRef {
    pub id: RawId,
    pub companion: CompanionId,
    /// Durable source message this entry reports on.
    pub source_message: RawId,
    pub status: ReportStatus,
    pub round: RawId,
    pub presence_generation: PresenceGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatus {
    Pending,
    Presented,
    /// Presentation unknown. Sticky: never upgraded by resend.
    PresentationUnknown,
}

/// An `Ok`-side domain outcome, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatusTransition {
    PendingToPresented,
    MarkedPresentationUnknown,
    /// The `expected` status was not current.
    StaleSource,
}

/// Presentation observation mapped to the undelivered boundary.
///
/// `presented` distinguishes presented (`true`) from unknown (`false`);
/// sending alone never marks an entry.
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
    /// append atom; task-sourced registration is a separate transaction
    /// after the task durable. The returned [`Option`] carries the
    /// registered [`UndeliveredRef`] when registration happened.
    async fn append_reply_with_undelivered(
        &self,
        cmd: AppendHistoryCommand,
        register_unpresented: bool,
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
}

/// Undelivered registration and reporting contract.
///
/// Registration happens only when the parent message is durable. Marking
/// happens only after a presentation observation; sending alone never marks
/// an entry as presented.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UndeliveredRepository {
    /// Compares `expected` against the current [`ReportStatus`] and, on
    /// match, applies `mark` in one atomic per-row compare.
    ///
    /// Mismatch returns `Ok(ReportStatusTransition::StaleSource)`, never
    /// `Err`.
    async fn compare_and_mark_reported(
        &self,
        id: RawId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;

    async fn list_pending(
        &self,
        companion: CompanionId,
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{
        AppendHistoryCommand, CommandId, CompanionId, HistoryMessage, HistoryRole, RoundIntentMark,
    };
    use ene_presence::PresenceGeneration;
    use ene_primitive::{RawId, WallClockWithTz};

    fn clock() -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
            .expect("fixture timestamp parses")
    }

    fn command() -> AppendHistoryCommand {
        AppendHistoryCommand {
            companion: CompanionId::from_raw(RawId::new()),
            round: RawId::new(),
            role: HistoryRole::Owner,
            text: String::from("private words"),
            lang: String::from("en"),
            at: clock(),
            expected_generation: PresenceGeneration::first(),
            expected_consent: None,
            expected_credential_set: None,
            expected_owner_message: None,
            command_id: Some(CommandId(RawId::new())),
            round_wire: Some(String::from("round-wire-1")),
            round_intent: Some(RoundIntentMark::Auto),
            incarnation: Some((1, 2)),
            local_id: Some(String::from("local-1")),
        }
    }

    fn message() -> HistoryMessage {
        HistoryMessage {
            id: RawId::new(),
            companion: CompanionId::from_raw(RawId::new()),
            round: RawId::new(),
            role: HistoryRole::Companion,
            text: String::from("private words"),
            lang: String::from("en"),
            at: clock(),
            presence_generation: PresenceGeneration::first(),
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        }
    }

    #[test]
    fn generated_companion_ids_differ() {
        assert_ne!(CompanionId::generate(), CompanionId::generate());
    }

    #[test]
    fn history_debug_redacts_text_and_keeps_refs() {
        let item = message();
        let rendered = format!("{item:?}");
        assert!(
            !rendered.contains("private words"),
            "text redacted: {rendered}"
        );
        assert!(rendered.contains("en"), "lang stays: {rendered}");
    }

    #[test]
    fn command_debug_redacts_text_and_keeps_lang() {
        let rendered = format!("{:?}", command());
        assert!(
            !rendered.contains("private words"),
            "text redacted: {rendered}"
        );
        assert!(rendered.contains("en"), "lang stays: {rendered}");
    }

    #[test]
    fn request_fingerprint_covers_request_semantics_only() {
        let mut keyed = command();
        keyed.command_id = Some(CommandId(RawId::new()));
        let fingerprint = keyed
            .request_fingerprint()
            .expect("a keyed command carries a request fingerprint");
        assert_eq!(fingerprint.round_intent, RoundIntentMark::Auto);
        let rendered = format!("{fingerprint:?}");
        assert!(
            !rendered.contains("private words"),
            "body redacted: {rendered}"
        );
        // Same key, same request semantics: equal.
        let mut same = keyed.clone();
        same.round = RawId::new();
        same.round_wire = Some(String::from("rotated"));
        same.local_id = Some(String::from("other-local"));
        same.at = clock();
        same.expected_generation = PresenceGeneration::from_u64(9);
        assert_eq!(
            keyed.request_fingerprint(),
            same.request_fingerprint(),
            "accepted-result and transport-only drift never decide replay"
        );
        // Round intent is request semantics: a flip must not fingerprint
        // equal, so a retry cannot adopt the changed intent.
        let mut joined = keyed.clone();
        joined.round_intent = Some(RoundIntentMark::Existing(String::from("round-wire-1")));
        assert_ne!(
            keyed.request_fingerprint(),
            joined.request_fingerprint(),
            "a changed round intent must change the fingerprint"
        );
        let mut keyless = keyed;
        keyless.command_id = None;
        assert_eq!(keyless.request_fingerprint(), None);
    }
}
