//! Usage cap definition, currentness, and cap-admission identity.
//!
//! `usage-cost-cap` §1/§6/§7/§13: permission / constraint owns whether a new
//! use may start, the daily/monthly limit values, and the current cap
//! revision. Inference owns the provider call's actual usage, pricing
//! snapshot, and cost fact; it may hold the [`UsageReservationRef`] the cap
//! admission boundary returned as durable attempt correlation, but it never
//! decides the limit or the admission by itself.
//!
//! Window boundaries are fixed to the UTC calendar, not to the client locale
//! or the process timezone at the moment of the query: a cap's consumption
//! must not move because the machine's local offset changed.

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveTime, TimeZone, Utc};
use ene_primitive::{Money, RawId, RevisionInner, WallClockWithTz};

use crate::PermissionTechnicalError;

/// What one cap limits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UsageCapScope {
    /// Every provider's usage in the window.
    System,
    /// Only this provider's usage in the window. The provider name matches
    /// the admission route exactly; there is no family or prefix match.
    Provider(String),
}

impl UsageCapScope {
    /// Stable storage name of the scope discriminant.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Provider(_) => "provider",
        }
    }

    /// The provider this scope names, or `None` for [`Self::System`].
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::System => None,
            Self::Provider(provider) => Some(provider),
        }
    }

    /// Rebuilds a stored scope. A system row must carry no provider and a
    /// provider row must name one; anything else is unreadable, never a
    /// guessed scope.
    #[must_use]
    pub fn from_stored(scope: &str, provider: &str) -> Option<Self> {
        match (scope, provider) {
            ("system", "") => Some(Self::System),
            ("provider", name) if !name.is_empty() => Some(Self::Provider(name.to_owned())),
            _ => None,
        }
    }
}

/// The UTC calendar window one cap's consumption is summed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageCapWindow {
    /// The UTC calendar day containing the admission instant.
    DailyUtc,
    /// The UTC calendar month containing the admission instant.
    MonthlyUtc,
}

impl UsageCapWindow {
    /// Stable storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DailyUtc => "daily_utc",
            Self::MonthlyUtc => "monthly_utc",
        }
    }

    /// Parses a stored name, closed world.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "daily_utc" => Some(Self::DailyUtc),
            "monthly_utc" => Some(Self::MonthlyUtc),
            _ => None,
        }
    }

    /// The half-open UTC period containing `at`: `start <= at < end`.
    ///
    /// `None` only at the representable-calendar extreme, where the next
    /// boundary cannot be constructed. Callers must fail closed there: a
    /// guessed window could attribute consumption to the wrong period.
    #[must_use]
    pub fn period_containing(self, at: WallClockWithTz) -> Option<UtcPeriod> {
        let utc = at.as_datetime().with_timezone(&Utc);
        let date = utc.date_naive();
        let (start, end) = match self {
            Self::DailyUtc => {
                let start = utc_day_start(date);
                (start, start.checked_add_days(Days::new(1))?)
            }
            Self::MonthlyUtc => {
                let month_start = date.with_day(1)?;
                let start = utc_day_start(month_start);
                let next = month_start.checked_add_months(Months::new(1))?;
                (start, utc_day_start(next))
            }
        };
        Some(UtcPeriod {
            start: WallClockWithTz::from_datetime(start.fixed_offset()),
            end: WallClockWithTz::from_datetime(end.fixed_offset()),
        })
    }
}

/// Midnight UTC of one calendar date.
fn utc_day_start(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
}

/// One half-open UTC calendar period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcPeriod {
    start: WallClockWithTz,
    end: WallClockWithTz,
}

impl UtcPeriod {
    /// Inclusive start instant.
    #[must_use]
    pub const fn start(self) -> WallClockWithTz {
        self.start
    }

    /// Exclusive end instant.
    #[must_use]
    pub const fn end(self) -> WallClockWithTz {
        self.end
    }

    /// Whether `at` falls inside this period.
    #[must_use]
    pub fn contains(self, at: WallClockWithTz) -> bool {
        self.start.as_datetime() <= at.as_datetime() && at.as_datetime() < self.end.as_datetime()
    }
}

/// Stable identity of one cap row: the scope it limits and the window it sums.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapId {
    scope: UsageCapScope,
    window: UsageCapWindow,
}

impl UsageCapId {
    /// Builds the identity of the one cap row for `(scope, window)`.
    #[must_use]
    pub const fn new(scope: UsageCapScope, window: UsageCapWindow) -> Self {
        Self { scope, window }
    }

    /// What the cap limits.
    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        &self.scope
    }

    /// The window the cap sums.
    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.window
    }
}

/// Monotonic content revision of one cap row.
///
/// Meaningful only together with its [`UsageCapId`]: a larger revision never
/// proves newness across different caps. [`Self::checked_next`] reports
/// exhaustion instead of aliasing `u64::MAX`, so a new revision can never
/// silently share the previous one's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UsageCapRevision(RevisionInner);

impl UsageCapRevision {
    /// The revision of a newly created cap row.
    #[must_use]
    pub fn first() -> Self {
        Self(RevisionInner::first())
    }

    /// Rebuilds a stored revision.
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(RevisionInner::from_u64(value))
    }

    /// The stored revision number.
    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.as_u64()
    }

    /// The successor revision, or `None` when the revision space is
    /// exhausted: the caller must refuse the update rather than writing
    /// `u64::MAX` again with new content.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

/// Currentness premise of one cap row: `(id, revision)` travel together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapRef {
    id: UsageCapId,
    revision: UsageCapRevision,
}

impl UsageCapRef {
    /// Builds the premise of one stored cap row.
    #[must_use]
    pub const fn new(id: UsageCapId, revision: UsageCapRevision) -> Self {
        Self { id, revision }
    }

    /// The cap this premise names.
    #[must_use]
    pub const fn id(&self) -> &UsageCapId {
        &self.id
    }

    /// What the cap limits.
    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        self.id.scope()
    }

    /// The window the cap sums.
    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.id.window()
    }

    /// The revision the premise names.
    #[must_use]
    pub const fn revision(&self) -> UsageCapRevision {
        self.revision
    }
}

/// One durable cap definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCap {
    id: UsageCapId,
    revision: UsageCapRevision,
    limit: Money,
}
impl UsageCap {
    /// Composes one stored cap.
    #[must_use]
    pub const fn new(id: UsageCapId, revision: UsageCapRevision, limit: Money) -> Self {
        Self {
            id,
            revision,
            limit,
        }
    }

    /// The cap's currentness premise.
    #[must_use]
    pub fn reference(&self) -> UsageCapRef {
        UsageCapRef::new(self.id.clone(), self.revision)
    }

    /// The identity (scope + window) this cap belongs to.
    #[must_use]
    pub const fn id(&self) -> &UsageCapId {
        &self.id
    }

    /// What the cap limits.
    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        self.id.scope()
    }

    /// The window the cap sums.
    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.id.window()
    }

    /// The current revision.
    #[must_use]
    pub const fn revision(&self) -> UsageCapRevision {
        self.revision
    }

    /// The maximum amount the window may consume.
    #[must_use]
    pub const fn limit(&self) -> Money {
        self.limit
    }
}

/// Command to create or update one cap.
///
/// `expected` is the caller's currentness premise: `None` believes no cap is
/// stored yet, `Some(reference)` names the exact `(id, revision)` the caller
/// read. A mismatch stores nothing and answers [`SetUsageCapOutcome::Stale`]
/// with the current row, so a cap update and a send admission serialize on
/// the same revision compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetUsageCapCommand {
    /// Currentness premise, or `None` when no cap is believed to exist.
    pub expected: Option<UsageCapRef>,
    /// What the cap limits.
    pub scope: UsageCapScope,
    /// The window the cap sums.
    pub window: UsageCapWindow,
    /// The maximum amount the window may consume.
    pub limit: Money,
}

/// Outcome of one cap create/update.
///
/// A domain outcome, never a technical error. `InvalidLimit` and `Stale`
/// store nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetUsageCapOutcome {
    /// The cap is stored at this reference.
    StoredAs(UsageCapRef),
    /// The caller's premise disagreed with the stored row; nothing was
    /// written. `current` is `None` when no cap is stored for this
    /// `(scope, window)`.
    Stale { current: Option<UsageCapRef> },
    /// The limit cannot be a meaningful ceiling: a zero limit is not a
    /// budget, and the caller must use an explicit stop rather than a cap
    /// that silently blocks every send. Nothing was written.
    InvalidLimit,
}

/// One current cap's consumption premise (`usage-cost-cap` §16): the window
/// sums an admission would compare against.
///
/// The three state sums are the durable reservation breakdown; `consumed` is
/// their exact sum and is what a new reservation is compared against.
/// `remaining` is the unspent limit (`0` once the limit is reached or
/// exceeded), and `held` says whether an admission would already refuse a
/// new send. Nothing here is estimated: a row whose currency disagrees with
/// the cap, whose period is unrepresentable, or whose sum overflows answers
/// [`Self::Indeterminate`], never a guessed total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageCapConsumption {
    Known {
        /// Reserved upper bounds of non-terminal reservations.
        reserved: Money,
        /// Actual committed cost of reported settlements.
        committed_reported: Money,
        /// Reserved upper bounds kept counted for unknown settlements.
        committed_unknown: Money,
        /// Exact sum of the three components.
        consumed: Money,
        /// Unspent limit: `limit - consumed`, or zero once held.
        remaining: Money,
        /// Whether the consumed amount already reaches the limit, i.e. the
        /// next admission cannot fit a positive reservation.
        held: bool,
    },
    /// The durable rows cannot answer a comparable amount (a mismatched
    /// currency, an unrepresentable period, or an overflowing sum). The
    /// caller must not render this as zero consumption, and no send is
    /// admitted under an indeterminate cap.
    Indeterminate,
}

/// One current cap plus its window consumption at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCapStatus {
    /// The stored cap definition and its currentness revision.
    pub cap: UsageCap,
    /// Current window consumption of exactly this cap's scope and window.
    pub consumption: UsageCapConsumption,
}

/// Bounded cap status query (`usage-cost-cap` §16).
///
/// `provider` narrows the provider-scoped rows (the system scope is always
/// included because it budgets every provider); `None` reports every stored
/// cap. The number of caps is bounded by the scope model (system plus one
/// pair of windows per provider), so this read has no pagination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCapStatusQuery {
    /// Provider whose scoped caps are reported, or every stored cap.
    pub provider: Option<String>,
    /// The instant the containing UTC day/month is computed from. The caller
    /// supplies it; a client clock never decides a cap window.
    pub at: WallClockWithTz,
}

/// Renders one cap's currentness mark:
/// `usage-cap-system-{window}-rev-N` / `...-none`, or
/// `usage-cap-provider-{provider}-{window}-rev-N` / `...-none`.
///
/// Single grammar owner: the Host renders the display mark from this
/// function and parses an intent's base view with
/// [`parse_usage_cap_mark`], so the two can never disagree on what a mark
/// names. The mark is opaque to the Client, which only echoes it.
#[must_use]
pub fn usage_cap_mark(
    scope: &UsageCapScope,
    window: UsageCapWindow,
    revision: Option<UsageCapRevision>,
) -> String {
    let scope = match scope {
        UsageCapScope::System => String::from("system"),
        UsageCapScope::Provider(provider) => format!("provider-{provider}"),
    };
    match revision {
        Some(revision) => format!(
            "usage-cap-{scope}-{}-rev-{}",
            window.as_str(),
            revision.as_u64()
        ),
        None => format!("usage-cap-{scope}-{}-none", window.as_str()),
    }
}

/// Parses the state one base-view mark names for exactly
/// `(scope, window)`: `Some(None)` expects no stored cap, `Some(Some(n))`
/// expects the cap at revision `n`, and `None` means the mark is stale on
/// its face (another cap, another shape, or unparseable). A stale face is
/// never treated as "expects nothing": only the exact `none` mark says that.
#[must_use]
pub fn parse_usage_cap_mark(
    mark: &str,
    scope: &UsageCapScope,
    window: UsageCapWindow,
) -> Option<Option<u64>> {
    let prefix = match scope {
        UsageCapScope::System => format!("usage-cap-system-{}-", window.as_str()),
        UsageCapScope::Provider(provider) => {
            format!("usage-cap-provider-{provider}-{}-", window.as_str())
        }
    };
    let state = mark.strip_prefix(&prefix)?;
    if state == "none" {
        return Some(None);
    }
    let revision = state.strip_prefix("rev-")?.parse::<u64>().ok()?;
    Some(Some(revision))
}

/// Cap definition owner boundary.
///
/// The command is the only mutation path; LLM output, Task Agent turns, and
/// provider responses never reach it. Raising a cap is a trusted first-party
/// control too: the expected revision compare is what lets a send admission
/// that already read the old revision commit first, and the raise then only
/// affects the next call.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageCapRepository: Send + Sync {
    /// Creates or updates one cap, compare-and-set on the expected revision.
    ///
    /// The compare and the write share one short `Immediate` transaction with
    /// the send admission path, so an admission that committed first is
    /// observed here (answering `Stale`) and an update that committed first
    /// is what the next admission reads.
    async fn set_usage_cap(
        &self,
        command: SetUsageCapCommand,
    ) -> Result<SetUsageCapOutcome, PermissionTechnicalError>;

    /// Reads the current cap status rows (`usage-cost-cap` §16).
    ///
    /// Every returned row carries the stored definition (identity, limit,
    /// current revision) and the consumption of its own UTC window at
    /// `query.at`, computed from the same durable reservation rows the send
    /// admission compares, so the displayed state and the admission can never
    /// disagree. SELECT-only: the read settles no reservation, creates no
    /// reservation, and mutates no cap. A row that cannot be compared answers
    /// [`UsageCapConsumption::Indeterminate`] rather than a guessed amount.
    async fn load_usage_cap_status(
        &self,
        query: UsageCapStatusQuery,
    ) -> Result<Vec<UsageCapStatus>, PermissionTechnicalError>;
}

/// Opaque durable identity of one usage reservation.
///
/// The cap-admission boundary (the attempt claim transaction) mints it;
/// inference stores it as attempt correlation. It is a domain identity, not
/// an estimate or an amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageReservationRef(pub RawId);

/// Lifecycle state of one usage reservation.
///
/// `Reserved` is non-terminal; the other three are terminal and are never
/// revised afterwards (`usage-cost-cap` §7.1). A reservation whose external
/// consumption cannot be denied settles [`Self::CommittedUnknown`], never
/// [`Self::Released`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageReservationState {
    /// The provider I/O may run; the upper bound is counted against every
    /// applicable cap.
    Reserved,
    /// The provider reported complete usage; cap accounting counts the
    /// actual committed cost and releases the unused reservation amount.
    CommittedReported,
    /// The external consumption cannot be denied (timeout, response lost,
    /// crash, transport interruption); cap accounting keeps the reserved
    /// upper bound.
    CommittedUnknown,
    /// Provider I/O provably never started; nothing is counted.
    Released,
}

impl UsageReservationState {
    /// Stable storage name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::CommittedReported => "committed_reported",
            Self::CommittedUnknown => "committed_unknown",
            Self::Released => "released",
        }
    }

    /// Parses a stored name, closed world.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "reserved" => Some(Self::Reserved),
            "committed_reported" => Some(Self::CommittedReported),
            "committed_unknown" => Some(Self::CommittedUnknown),
            "released" => Some(Self::Released),
            _ => None,
        }
    }

    /// Whether the state can no longer change.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Reserved)
    }
}
