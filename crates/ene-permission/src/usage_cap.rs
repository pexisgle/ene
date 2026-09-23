use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveTime, TimeZone, Utc};
use ene_primitive::{Money, RawId, RevisionInner, WallClockWithTz};

use crate::PermissionTechnicalError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UsageCapScope {
    System,
    Provider(String),
}

impl UsageCapScope {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Provider(_) => "provider",
        }
    }

    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::System => None,
            Self::Provider(provider) => Some(provider),
        }
    }

    #[must_use]
    pub fn from_stored(scope: &str, provider: &str) -> Option<Self> {
        match (scope, provider) {
            ("system", "") => Some(Self::System),
            ("provider", name) if !name.is_empty() => Some(Self::Provider(name.to_owned())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageCapWindow {
    DailyUtc,
    MonthlyUtc,
}

impl UsageCapWindow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DailyUtc => "daily_utc",
            Self::MonthlyUtc => "monthly_utc",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "daily_utc" => Some(Self::DailyUtc),
            "monthly_utc" => Some(Self::MonthlyUtc),
            _ => None,
        }
    }

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

fn utc_day_start(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcPeriod {
    start: WallClockWithTz,
    end: WallClockWithTz,
}

impl UtcPeriod {
    #[must_use]
    pub const fn start(self) -> WallClockWithTz {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> WallClockWithTz {
        self.end
    }

    #[must_use]
    pub fn contains(self, at: WallClockWithTz) -> bool {
        self.start.as_datetime() <= at.as_datetime() && at.as_datetime() < self.end.as_datetime()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapId {
    scope: UsageCapScope,
    window: UsageCapWindow,
}

impl UsageCapId {
    #[must_use]
    pub const fn new(scope: UsageCapScope, window: UsageCapWindow) -> Self {
        Self { scope, window }
    }

    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        &self.scope
    }

    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UsageCapRevision(RevisionInner);

impl UsageCapRevision {
    #[must_use]
    pub fn first() -> Self {
        Self(RevisionInner::first())
    }

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(RevisionInner::from_u64(value))
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.as_u64()
    }

    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageCapRef {
    id: UsageCapId,
    revision: UsageCapRevision,
}

impl UsageCapRef {
    #[must_use]
    pub const fn new(id: UsageCapId, revision: UsageCapRevision) -> Self {
        Self { id, revision }
    }

    #[must_use]
    pub const fn id(&self) -> &UsageCapId {
        &self.id
    }

    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        self.id.scope()
    }

    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.id.window()
    }

    #[must_use]
    pub const fn revision(&self) -> UsageCapRevision {
        self.revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCap {
    id: UsageCapId,
    revision: UsageCapRevision,
    limit: Money,
}
impl UsageCap {
    #[must_use]
    pub const fn new(id: UsageCapId, revision: UsageCapRevision, limit: Money) -> Self {
        Self {
            id,
            revision,
            limit,
        }
    }

    #[must_use]
    pub fn reference(&self) -> UsageCapRef {
        UsageCapRef::new(self.id.clone(), self.revision)
    }

    #[must_use]
    pub const fn id(&self) -> &UsageCapId {
        &self.id
    }

    #[must_use]
    pub const fn scope(&self) -> &UsageCapScope {
        self.id.scope()
    }

    #[must_use]
    pub const fn window(&self) -> UsageCapWindow {
        self.id.window()
    }

    #[must_use]
    pub const fn revision(&self) -> UsageCapRevision {
        self.revision
    }

    #[must_use]
    pub const fn limit(&self) -> Money {
        self.limit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetUsageCapCommand {
    pub expected: Option<UsageCapRef>,
    pub scope: UsageCapScope,
    pub window: UsageCapWindow,
    pub limit: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetUsageCapOutcome {
    StoredAs(UsageCapRef),
    Stale { current: Option<UsageCapRef> },
    InvalidLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageCapConsumption {
    Known {
        reserved: Money,
        committed_reported: Money,
        committed_unknown: Money,
        consumed: Money,
        remaining: Money,
        held: bool,
    },
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCapStatus {
    pub cap: UsageCap,
    pub consumption: UsageCapConsumption,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCapStatusQuery {
    pub provider: Option<String>,
    pub at: WallClockWithTz,
}

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

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageCapRepository: Send + Sync {
    async fn set_usage_cap(
        &self,
        command: SetUsageCapCommand,
    ) -> Result<SetUsageCapOutcome, PermissionTechnicalError>;

    async fn load_usage_cap_status(
        &self,
        query: UsageCapStatusQuery,
    ) -> Result<Vec<UsageCapStatus>, PermissionTechnicalError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageReservationRef(pub RawId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageReservationState {
    Reserved,
    CommittedReported,
    CommittedUnknown,
    Released,
}

impl UsageReservationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::CommittedReported => "committed_reported",
            Self::CommittedUnknown => "committed_unknown",
            Self::Released => "released",
        }
    }

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

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Reserved)
    }
}
