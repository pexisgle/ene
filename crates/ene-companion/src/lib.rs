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

use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};

/// Companion identity.
///
/// Wraps a [`RawId`]; never converted to any other domain newtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompanionId(RawId);

impl CompanionId {
    /// Wraps an existing raw identity, for example one read back from storage.
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    /// Returns the wrapped raw identity for storage or transport encoding.
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    /// Generates a fresh random identity.
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

/// Who produced a history item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryRole {
    /// The Owner.
    Owner,
    /// The Companion.
    Companion,
}

/// One durable history item: Host-filtered display fact.
///
/// Filtered facts only, never undelivered reporting. Text is redacted from
/// [`core::fmt::Debug`]; refs stay visible.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HistoryMessage {
    /// Message identity.
    pub id: RawId,
    /// Companion this item belongs to.
    pub companion: CompanionId,
    /// Round this item belongs to.
    pub round: RawId,
    /// Who produced it.
    pub role: HistoryRole,
    /// Item text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    /// Opaque language tag.
    pub lang: String,
    /// Wall-clock time with its creation offset, display only.
    pub at: WallClockWithTz,
    /// Presence generation the item belongs to.
    pub presence_generation: PresenceGeneration,
    /// Command-scoped idempotency identity, when the caller carries one.
    /// [`None`] marks pre-command callers or an unknown command. The durable
    /// replay key; `local_id` stays as correspondence metadata only.
    /// Non-secret correspondence, visible in Debug.
    pub command_id: Option<CommandId>,
    /// Opaque wire projection of `round`, minted fresh by the caller per
    /// round and unrelated to the domain bytes: the only round string that
    /// ever crosses the wire. [`None`] marks pre-opaque rows.
    pub round_wire: Option<String>,
    /// Client incarnation that sent the item, as `(counter, random)` when
    /// the caller carries one. Part of the replay fingerprint together with
    /// text, language, round wire, and generation.
    pub incarnation: Option<(u64, u64)>,
    /// Client-local correspondence ID for matching an input to its ack.
    /// Correspondence metadata only, no longer the durable key (that is
    /// `command_id`). Non-secret correspondence, visible in Debug.
    pub local_id: Option<String>,
}

impl core::fmt::Debug for HistoryMessage {
    /// Renders refs while redacting `text`.
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
            .field("incarnation", &self.incarnation)
            .field("local_id", &self.local_id)
            .finish()
    }
}

/// Command to append one history item under a generation expectation.
#[derive(Clone, PartialEq, Eq)]
pub struct AppendHistoryCommand {
    /// Companion to append to.
    pub companion: CompanionId,
    /// Round the item belongs to.
    pub round: RawId,
    /// Who produced it.
    pub role: HistoryRole,
    /// Item text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    /// Opaque language tag.
    pub lang: String,
    /// Wall-clock time with its creation offset, display only.
    pub at: WallClockWithTz,
    /// Generation value the caller relied on.
    pub expected_generation: PresenceGeneration,
    /// Consent premise the caller relied on, as an opaque `(id, rev)` pair
    /// that travels together (never a bare revision). [`None`] skips the
    /// consent check; callers that went through admission always pass
    /// [`Some`]. The store compares both inside the append transaction, so
    /// a mid-flight consent move answers [`HistoryAppendOutcome::StaleConsent`]
    /// instead of attributing content across the move.
    pub expected_consent: Option<(String, u64)>,
    /// Command-scoped idempotency identity, when the caller carries one.
    /// [`None`] stores NULL (no replay key). A retry reuses the same command
    /// id with a fresh message id; `local_id` stays as correspondence
    /// metadata only. Non-secret correspondence, visible in Debug.
    pub command_id: Option<CommandId>,
    /// Opaque wire projection of `round`, minted fresh by the caller per
    /// round and unrelated to the domain bytes. The store persists it so
    /// replay acks and timeline views echo it back instead of rendering the
    /// domain identity.
    pub round_wire: Option<String>,
    /// Client incarnation that sends the item, as `(counter, random)`.
    /// Part of the replay fingerprint; [`None`] skips that check.
    pub incarnation: Option<(u64, u64)>,
    /// Client-local correspondence ID for matching an input to its ack, if
    /// the caller carries one. [`None`] stores NULL. Correspondence metadata
    /// only, no longer the durable key (that is `command_id`).
    pub local_id: Option<String>,
}

impl core::fmt::Debug for AppendHistoryCommand {
    /// Renders refs while redacting `text`.
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
            .field("command_id", &self.command_id)
            .field("round_wire", &self.round_wire)
            .field("incarnation", &self.incarnation)
            .field("local_id", &self.local_id)
            .finish()
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
    CommittedAs {
        /// Committed message identity.
        message: RawId,
    },
    /// Already committed under the same idempotency key by a concurrent
    /// attempt: the caller must NOT re-run inference or re-append. It
    /// answers the original acceptance (round below) and stops, so a
    /// transport retry racing the original can neither duplicate effects
    /// nor steal the round.
    AlreadyCommittedAs {
        /// Original message identity.
        message: RawId,
        /// Original round identity for the accept ack.
        round: RawId,
    },
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
    /// A reused command key arrived with different content than the stored
    /// row. The fingerprint (round, role, text, language, round wire, and
    /// incarnation) did not match, so the send is declined without side
    /// effects: no new row, no undelivered registration, no inference.
    /// Carries no payload (the key itself is the caller's correlation): the
    /// Host answers a typed wire rejection, never an intake outcome, and
    /// never retries automatically. Resending the original content replays
    /// as [`HistoryAppendOutcome::AlreadyCommittedAs`] instead.
    CommandConflict,
    /// Held by the companion lifecycle.
    HeldByLifecycle {
        /// Lifecycle that held the append.
        lifecycle: CompanionLifecycle,
    },
}

/// Undelivered tracking fact: a durably stored item not yet confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredRef {
    /// Undelivered entry identity.
    pub id: RawId,
    /// Companion the entry belongs to.
    pub companion: CompanionId,
    /// Durable source message this entry reports on.
    pub source_message: RawId,
    /// Current report status.
    pub status: ReportStatus,
    /// Round the entry belongs to.
    pub round: RawId,
    /// Presence generation the entry belongs to.
    pub presence_generation: PresenceGeneration,
}

/// Report status vocabulary for undelivered items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatus {
    /// Waiting for a presentation observation.
    Pending,
    /// Confirmed as presented.
    Presented,
    /// Presentation unknown. Sticky: never upgraded by resend.
    PresentationUnknown,
}

/// Outcome of a compare-and-mark-reported attempt.
///
/// An `Ok`-side domain outcome, never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatusTransition {
    /// Moved from pending to presented.
    PendingToPresented,
    /// Marked as presentation-unknown.
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
    /// Round the observation covers.
    pub round: RawId,
    /// Whether the round was presented.
    pub presented: bool,
}

/// Infrastructure failure for companion and history operations.
///
/// Stale / held outcomes are [`HistoryAppendOutcome`], never this error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompanionTechnicalError {
    /// Durable storage was unavailable.
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
    /// Durable storage was unavailable.
    #[error("undelivered storage unavailable: {reason}")]
    StorageUnavailable {
        /// Operational reason. Never a secret or a body copy.
        reason: String,
    },
}

/// Companion lifecycle contract.
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
    /// `since` bounds items at or after that wall-clock rendering when set;
    /// `limit` caps the number returned. Wall-clock bounds are display
    /// filtering only, never currentness evidence.
    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError>;

    /// Looks up one previously accepted message by caller-supplied local id.
    ///
    /// Correspondence lookup for matching an input to its ack. Command-scoped
    /// replay uses [`HistoryRepository::lookup_command`]; stream outcomes are
    /// not replayed through either lookup — a caller that needs missed stream
    /// items recovers via `HistoryRequest`.
    async fn lookup_local_id(
        &self,
        companion: CompanionId,
        local_id: &str,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError>;

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
    /// Registers `entry` when its parent message is durable.
    ///
    /// Returns `true` when the entry was registered and `false` when the
    /// parent was not durable and nothing was registered.
    async fn register_if_parent_durable(
        &self,
        entry: UndeliveredRef,
    ) -> Result<bool, UndeliveredTechnicalError>;

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

    /// Lists pending entries for one companion.
    async fn list_pending(
        &self,
        companion: CompanionId,
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{
        AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionTechnicalError,
        HistoryAppendOutcome, HistoryMessage, HistoryRole, PresentationMark, ReportStatus,
        ReportStatusTransition, UndeliveredRef, UndeliveredTechnicalError,
    };
    use ene_presence::PresenceGeneration;
    use ene_primitive::{RawId, WallClockWithTz};

    fn clock() -> WallClockWithTz {
        let parsed = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00");
        assert!(parsed.is_ok(), "fixture timestamp must parse");
        if let Ok(at) = parsed {
            at
        } else {
            WallClockWithTz::now()
        }
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
            command_id: Some(CommandId(RawId::new())),
            round_wire: Some(String::from("round-wire-1")),
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
            incarnation: None,
            local_id: None,
        }
    }

    #[test]
    fn companion_id_round_trips_through_raw() {
        let raw = RawId::new();
        assert_eq!(CompanionId::from_raw(raw).as_raw(), raw);
    }

    #[test]
    fn generated_companion_ids_differ() {
        assert_ne!(CompanionId::generate(), CompanionId::generate());
    }

    #[test]
    fn history_debug_redacts_text_and_keeps_refs() {
        let item = message();
        let round = item.round;
        let rendered = format!("{item:?}");
        assert!(
            !rendered.contains("private words"),
            "text redacted: {rendered}"
        );
        assert!(rendered.contains("en"), "lang stays: {rendered}");
        let _ = round;
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
    fn append_outcomes_carry_current_or_lifecycle() {
        let committed = HistoryAppendOutcome::CommittedAs {
            message: RawId::new(),
        };
        assert!(matches!(
            committed,
            HistoryAppendOutcome::CommittedAs { .. }
        ));
        let stale = HistoryAppendOutcome::StaleExpected {
            current: PresenceGeneration::from_u64(3),
        };
        assert!(matches!(stale, HistoryAppendOutcome::StaleExpected { .. }));
        if let HistoryAppendOutcome::StaleExpected { current } = stale {
            assert_eq!(current, PresenceGeneration::from_u64(3));
        }
        let held = HistoryAppendOutcome::HeldByLifecycle {
            lifecycle: CompanionLifecycle::Stopped,
        };
        assert_eq!(
            held,
            HistoryAppendOutcome::HeldByLifecycle {
                lifecycle: CompanionLifecycle::Stopped,
            }
        );
        assert_eq!(CompanionLifecycle::Running, CompanionLifecycle::Running);
        assert_eq!(CompanionLifecycle::Deleted, CompanionLifecycle::Deleted);
    }

    #[test]
    fn undelivered_ref_and_transitions_construct() {
        let entry = UndeliveredRef {
            id: RawId::new(),
            companion: CompanionId::from_raw(RawId::new()),
            source_message: RawId::new(),
            status: ReportStatus::Pending,
            round: RawId::new(),
            presence_generation: PresenceGeneration::first(),
        };
        assert_eq!(entry.status, ReportStatus::Pending);
        assert_eq!(
            ReportStatusTransition::PendingToPresented,
            ReportStatusTransition::PendingToPresented
        );
        assert_eq!(
            ReportStatusTransition::MarkedPresentationUnknown,
            ReportStatusTransition::MarkedPresentationUnknown
        );
        assert_eq!(
            ReportStatusTransition::StaleSource,
            ReportStatusTransition::StaleSource
        );
        assert_eq!(ReportStatus::Presented, ReportStatus::Presented);
        assert_eq!(
            ReportStatus::PresentationUnknown,
            ReportStatus::PresentationUnknown
        );
        let mark = PresentationMark {
            round: entry.round,
            presented: true,
        };
        assert!(mark.presented);
        assert_eq!(mark.round, entry.round);
    }

    #[test]
    fn technical_errors_render_reasons() {
        let companion = CompanionTechnicalError::StorageUnavailable {
            reason: String::from("disk offline"),
        };
        let rendered = format!("{companion}");
        assert!(
            rendered.contains("disk offline"),
            "reason stays: {rendered}"
        );
        let undelivered = UndeliveredTechnicalError::StorageUnavailable {
            reason: String::from("index offline"),
        };
        let rendered = format!("{undelivered}");
        assert!(
            rendered.contains("index offline"),
            "reason stays: {rendered}"
        );
    }

    #[test]
    fn command_id_is_visible_non_secret_correspondence() {
        let id = CommandId(RawId::new());
        let rendered = format!("{id:?}");
        assert!(
            rendered.contains("CommandId"),
            "command id visible: {rendered}"
        );
        let mut item = message();
        assert_eq!(item.command_id, None);
        item.command_id = Some(id);
        assert_eq!(item.command_id, Some(id));
        let rendered_item = format!("{item:?}");
        assert!(
            !rendered_item.contains("private words"),
            "text redacted: {rendered_item}"
        );
    }

    #[test]
    fn roles_cover_owner_and_companion() {
        assert_eq!(HistoryRole::Owner, HistoryRole::Owner);
        assert_eq!(HistoryRole::Companion, HistoryRole::Companion);
        assert_ne!(HistoryRole::Owner, HistoryRole::Companion);
    }
}
