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

    /// What the cap limits.
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

    /// What the cap limits.
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
    super::render_revision_state(
        &format!("usage-cap-{scope}-{}-", window.as_str()),
        revision.map(|revision| revision.as_u64()),
    )
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
    super::parse_revision_state(state)
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

#[cfg(test)]
mod tests {
    use super::{UsageCapScope, UsageCapWindow};
    use ene_primitive::WallClockWithTz;

    fn at(value: &str) -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339(value).expect("the fixture instant parses")
    }

    #[test]
    fn daily_window_is_the_utc_calendar_day_half_open() {
        let window = UsageCapWindow::DailyUtc;
        let period = window
            .period_containing(at("2026-03-15T13:45:00+09:00"))
            .expect("the period exists");
        // 2026-03-15T04:45:00Z: the local offset does not move the boundary.
        assert_eq!(period.start(), at("2026-03-15T00:00:00Z"));
        assert_eq!(period.end(), at("2026-03-16T00:00:00Z"));
    }

    #[test]
    fn monthly_window_is_the_utc_calendar_month_half_open() {
        let window = UsageCapWindow::MonthlyUtc;
        let period = window
            .period_containing(at("2026-03-15T23:59:00-05:00"))
            .expect("the period exists");
        assert_eq!(period.start(), at("2026-03-01T00:00:00Z"));
        assert_eq!(period.end(), at("2026-04-01T00:00:00Z"));
    }

    #[test]
    fn december_and_leap_boundaries_do_not_roll_over_the_wrong_way() {
        let monthly = UsageCapWindow::MonthlyUtc;
        let december = monthly
            .period_containing(at("2026-12-31T23:59:59Z"))
            .expect("the period exists");
        assert_eq!(december.end(), at("2027-01-01T00:00:00Z"));
        let february = monthly
            .period_containing(at("2028-02-29T12:00:00Z"))
            .expect("the period exists");
        assert_eq!(february.start(), at("2028-02-01T00:00:00Z"));
        assert_eq!(february.end(), at("2028-03-01T00:00:00Z"));
    }

    #[test]
    fn scope_storage_vocabulary_is_closed_world() {
        assert_eq!(UsageCapScope::System.as_str(), "system");
        assert_eq!(
            UsageCapScope::Provider(String::from("openai")).as_str(),
            "provider"
        );
        assert_eq!(
            UsageCapScope::from_stored("system", ""),
            Some(UsageCapScope::System)
        );
        assert_eq!(
            UsageCapScope::from_stored("provider", "openai"),
            Some(UsageCapScope::Provider(String::from("openai")))
        );
        assert_eq!(UsageCapScope::from_stored("system", "openai"), None);
        assert_eq!(UsageCapScope::from_stored("provider", ""), None);
        assert_eq!(UsageCapScope::from_stored("global", "openai"), None);
        assert_eq!(
            UsageCapWindow::from_name("daily_utc"),
            Some(UsageCapWindow::DailyUtc)
        );
        assert_eq!(
            UsageCapWindow::from_name("monthly_utc"),
            Some(UsageCapWindow::MonthlyUtc)
        );
        assert_eq!(UsageCapWindow::from_name("weekly_utc"), None);
    }

    #[test]
    fn cap_marks_roundtrip_per_slot_and_face_stale_otherwise() {
        use super::{UsageCapRevision, parse_usage_cap_mark, usage_cap_mark};
        let system = UsageCapScope::System;
        let openai = UsageCapScope::Provider(String::from("openai"));
        assert_eq!(
            usage_cap_mark(&system, UsageCapWindow::DailyUtc, None),
            "usage-cap-system-daily_utc-none"
        );
        assert_eq!(
            usage_cap_mark(
                &system,
                UsageCapWindow::MonthlyUtc,
                Some(UsageCapRevision::from_u64(3))
            ),
            "usage-cap-system-monthly_utc-rev-3"
        );
        assert_eq!(
            usage_cap_mark(
                &openai,
                UsageCapWindow::DailyUtc,
                Some(UsageCapRevision::from_u64(12))
            ),
            "usage-cap-provider-openai-daily_utc-rev-12"
        );
        assert_eq!(
            usage_cap_mark(&openai, UsageCapWindow::DailyUtc, None),
            "usage-cap-provider-openai-daily_utc-none"
        );
        for (mark, scope, window, expected) in [
            (
                "usage-cap-system-daily_utc-none",
                &system,
                UsageCapWindow::DailyUtc,
                Some(None),
            ),
            (
                "usage-cap-system-daily_utc-rev-3",
                &system,
                UsageCapWindow::DailyUtc,
                Some(Some(3)),
            ),
            (
                "usage-cap-provider-openai-daily_utc-rev-12",
                &openai,
                UsageCapWindow::DailyUtc,
                Some(Some(12)),
            ),
            (
                "usage-cap-provider-openai-daily_utc-none",
                &openai,
                UsageCapWindow::DailyUtc,
                Some(None),
            ),
        ] {
            assert_eq!(
                parse_usage_cap_mark(mark, scope, window),
                expected,
                "the mark roundtrips: {mark}"
            );
        }
        // Face-stale: another slot, another shape, a bad revision, or empty.
        for mark in [
            "usage-cap-system-monthly_utc-rev-3",
            "usage-cap-provider-other-daily_utc-rev-1",
            "usage-cap-system-daily_utc",
            "usage-cap-system-daily_utc-rev-x",
            "usage-cap-system-daily_utc-none-extra",
            "consent-dialogue-rev-1",
            "",
        ] {
            assert_eq!(
                parse_usage_cap_mark(mark, &system, UsageCapWindow::DailyUtc),
                None,
                "a foreign mark must be face-stale, not a guess: {mark}"
            );
        }
        // A provider name containing `-` still parses: the prefix is built
        // from the exact provider the reader asked about.
        let hyphenated = UsageCapScope::Provider(String::from("open-ai"));
        assert_eq!(
            parse_usage_cap_mark(
                "usage-cap-provider-open-ai-daily_utc-rev-1",
                &hyphenated,
                UsageCapWindow::DailyUtc
            ),
            Some(Some(1))
        );
    }
}
