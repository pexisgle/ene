pub mod dialogue;
use ene_credential::CredentialSetRevision;
use ene_presence::PresenceGeneration;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{TaskPurposeRef, TaskRef};

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
}

/// Command-scoped idempotency identity for history appends.
///
/// Carries a public [`RawId`]: the wire `CommandWireId` maps 1:1 at ingress
/// when the Host parses its UUID text into this domain newtype. The client
/// mints one per send; a transport retry reuses the same command id with a
/// fresh message id. Non-secret correspondence, visible in
/// [`core::fmt::Debug`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub RawId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RoundIntentMark {
    Auto,
    New,
    Existing(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompanionLifecycle {
    Running,
    Stopped,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryRole {
    Owner,
    Companion,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct HistoryMessage {
    pub id: RawId,
    pub companion: CompanionId,
    pub round: RawId,
    pub role: HistoryRole,
    pub text: String,
    pub lang: String,
    pub at: WallClockWithTz,
    pub presence_generation: PresenceGeneration,
    pub command_id: Option<CommandId>,
    pub round_wire: Option<String>,
    pub round_intent: Option<RoundIntentMark>,
    pub incarnation: Option<(u64, u64)>,
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
    pub text: String,
    pub lang: String,
    pub at: WallClockWithTz,
    pub expected_generation: PresenceGeneration,
    pub expected_consent: Option<(String, u64)>,
    pub expected_credential_set: Option<CredentialSetRevision>,
    pub expected_owner_message: Option<RawId>,
    pub command_id: Option<CommandId>,
    pub round_wire: Option<String>,
    pub round_intent: Option<RoundIntentMark>,
    pub incarnation: Option<(u64, u64)>,
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

/// Builds the request semantics fingerprint shared by both constructors.
///
/// Returns [`None`] exactly when the request carries no replay key or no
/// round intent: there is then nothing provable to compare. Keeping the
/// construction in one place stops the store's in-transaction judge and the
/// Host's early replay judge from drifting apart when a field is added.
fn fingerprint_of(
    command_id: Option<&CommandId>,
    role: HistoryRole,
    text: &str,
    lang: &str,
    incarnation: Option<(u64, u64)>,
    round_intent: Option<&RoundIntentMark>,
) -> Option<RequestFingerprint> {
    command_id?;
    Some(RequestFingerprint {
        role,
        text: text.to_owned(),
        lang: lang.to_owned(),
        incarnation,
        round_intent: round_intent.cloned()?,
    })
}

impl AppendHistoryCommand {
    #[must_use]
    pub fn request_fingerprint(&self) -> Option<RequestFingerprint> {
        fingerprint_of(
            self.command_id.as_ref(),
            self.role,
            &self.text,
            &self.lang,
            self.incarnation,
            self.round_intent.as_ref(),
        )
    }
}

impl HistoryMessage {
    #[must_use]
    pub fn request_fingerprint(&self) -> Option<RequestFingerprint> {
        fingerprint_of(
            self.command_id.as_ref(),
            self.role,
            &self.text,
            &self.lang,
            self.incarnation,
            self.round_intent.as_ref(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistoryAppendOutcome {
    CommittedAs { message: RawId },
    AlreadyCommittedAs { message: RawId, round: RawId },
    StaleExpected { current: PresenceGeneration },
    StaleConsent,
    StaleCredentialSet,
    StaleOwnerInput,
    CommandConflict,
    HeldByLifecycle { lifecycle: CompanionLifecycle },
    HeldForErasure,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RequestFingerprint {
    pub role: HistoryRole,
    pub text: String,
    pub lang: String,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCertaintyWire {
    ConfirmedSuccess,
    ConfirmedFailure,
    Unknown,
}

impl ActionCertaintyWire {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalKindWire {
    Failed,
    Cancelled,
}

impl TerminalKindWire {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskFact {
    TaskRevision {
        task: RawId,
        revision: u64,
    },
    Delegation(RawId),
    ActionAttempt {
        attempt: RawId,
        certainty: ActionCertaintyWire,
    },
    ResultRecorded(RawId),
    ResultAdopted(RawId),
    Terminal {
        task: RawId,
        progress: TerminalKindWire,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UndeliveredSource {
    TaskRecord { task: RawId, fact: TaskFact },
    HistoryMessage(RawId),
    ActivityRecord(RawId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredRef {
    pub id: UndeliveredId,
    pub companion: CompanionId,
    pub source: UndeliveredSource,
    pub status: ReportStatus,
    pub created_at: WallClockWithTz,
    pub round: Option<RawId>,
    pub presence_generation: Option<PresenceGeneration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatus {
    Pending,
    PresentationUnknown,
    Presented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportStatusTransition {
    PendingToPresented,
    MarkedPresentationUnknown,
    AlreadyPresented,
    FailedToPending,
    StaleSource,
    HeldForErasure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeliveredPage {
    pub entries: Vec<UndeliveredRef>,
    pub next: Option<UndeliveredCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UndeliveredCursor {
    after_seq: u64,
    pass_upper_bound: u64,
}

impl UndeliveredCursor {
    #[must_use]
    pub const fn begin(after_seq: u64, pass_upper_bound: u64) -> Self {
        Self {
            after_seq,
            pass_upper_bound,
        }
    }

    #[must_use]
    pub const fn after_seq(self) -> u64 {
        self.after_seq
    }

    #[must_use]
    pub const fn pass_upper_bound(self) -> u64 {
        self.pass_upper_bound
    }
}

pub const UNDELIVERED_PAGE_MAX: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationMark {
    pub round: RawId,
    pub presented: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompanionTechnicalError {
    #[error("companion storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UndeliveredTechnicalError {
    #[error("undelivered storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CompanionRepository {
    async fn ensure_running_companion(&self) -> Result<CompanionId, CompanionTechnicalError>;

    async fn load_lifecycle(
        &self,
        companion: CompanionId,
    ) -> Result<Option<CompanionLifecycle>, CompanionTechnicalError>;
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait HistoryRepository {
    async fn append_message(
        &self,
        cmd: AppendHistoryCommand,
    ) -> Result<HistoryAppendOutcome, CompanionTechnicalError>;

    /// Appends one companion reply and registers an undelivered entry for it
    /// in the same atomic section.
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
        inference_claim: Option<RawId>,
    ) -> Result<(HistoryAppendOutcome, Option<UndeliveredRef>), CompanionTechnicalError>;

    async fn load_timeline(
        &self,
        companion: CompanionId,
        since: Option<WallClockWithTz>,
        round: Option<RawId>,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError>;

    async fn load_recent_timeline(
        &self,
        companion: CompanionId,
        limit: u64,
    ) -> Result<Vec<HistoryMessage>, CompanionTechnicalError>;

    async fn lookup_command(
        &self,
        companion: CompanionId,
        command: &CommandId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError>;

    async fn load_message(
        &self,
        message: RawId,
    ) -> Result<Option<HistoryMessage>, CompanionTechnicalError>;
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UndeliveredRepository {
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;

    async fn undelivered_pass_bound(&self) -> Result<u64, UndeliveredTechnicalError>;

    async fn list_unpresented(
        &self,
        companion: CompanionId,
        cursor: Option<UndeliveredCursor>,
        limit: u32,
    ) -> Result<UndeliveredPage, UndeliveredTechnicalError>;

    async fn load_undelivered_by_ids(
        &self,
        companion: CompanionId,
        ids: &[UndeliveredId],
    ) -> Result<Vec<UndeliveredRef>, UndeliveredTechnicalError>;
}

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

#[derive(Clone, PartialEq, Eq)]
pub struct ManagementActivity {
    pub id: ActivityId,
    pub companion: CompanionId,
    pub task: TaskRef,
    pub purpose: TaskPurposeRef,
    pub body: String,
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

#[derive(Clone, PartialEq, Eq)]
pub struct RecordResumeActivityCommand {
    pub companion: CompanionId,
    pub task: TaskRef,
    pub purpose: TaskPurposeRef,
    pub body: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeActivityOutcome {
    Recorded(ActivityId),
    HeldForErasure,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait ActivityRepository {
    async fn record_resume_activity(
        &self,
        cmd: RecordResumeActivityCommand,
    ) -> Result<ResumeActivityOutcome, CompanionTechnicalError>;

    async fn load_activity(
        &self,
        activity: ActivityId,
    ) -> Result<Option<ManagementActivity>, CompanionTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{
        ActionCertaintyWire, AppendHistoryCommand, CommandId, CompanionId, HistoryMessage,
        HistoryRole, RoundIntentMark, TerminalKindWire,
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
    fn undelivered_wire_names_round_trip_as_a_closed_world() {
        for certainty in [
            ActionCertaintyWire::ConfirmedSuccess,
            ActionCertaintyWire::ConfirmedFailure,
            ActionCertaintyWire::Unknown,
        ] {
            assert_eq!(
                ActionCertaintyWire::from_name(certainty.as_str()),
                Some(certainty)
            );
        }
        assert_eq!(ActionCertaintyWire::from_name("confirmed"), None);
        for kind in [TerminalKindWire::Failed, TerminalKindWire::Cancelled] {
            assert_eq!(TerminalKindWire::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(TerminalKindWire::from_name("completed"), None);
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
