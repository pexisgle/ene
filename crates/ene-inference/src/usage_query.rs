use ene_permission::{ConsumerKind, PurposeKind};
use ene_primitive::WallClockWithTz;

use crate::InferenceTechnicalError;
use crate::InferenceTicketId;
use crate::cost::{Money, UsageCostFact};

pub const USAGE_SUMMARY_PAGE_MAX: u32 = 50;

pub const USAGE_SUMMARY_RANGE_MAX_DAYS: u64 = 366;

pub const USAGE_SUMMARY_RANGE_DEFAULT_DAYS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageSummaryStatus {
    Reported,
    Unknown,
    Reserved,
}

impl UsageSummaryStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Unknown => "unknown",
            Self::Reserved => "reserved",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "reported" => Some(Self::Reported),
            "unknown" => Some(Self::Unknown),
            "reserved" => Some(Self::Reserved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportedTokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummaryRow {
    pub ticket: InferenceTicketId,
    pub provider: String,
    pub model: String,
    pub consumer: ConsumerKind,
    pub purpose: PurposeKind,
    pub status: UsageSummaryStatus,
    pub tokens: Option<ReportedTokenUsage>,
    pub cost: Option<UsageCostFact>,
    pub reserved: Option<Money>,
    pub started_at: WallClockWithTz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageSummaryCursor {
    pub started_at: WallClockWithTz,
    pub ticket: InferenceTicketId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummaryQuery {
    pub from: WallClockWithTz,
    pub to: WallClockWithTz,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub consumer: Option<ConsumerKind>,
    pub purpose: Option<PurposeKind>,
    pub status: Option<UsageSummaryStatus>,
    pub after: Option<UsageSummaryCursor>,
    pub limit: u32,
}

impl UsageSummaryQuery {
    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        self.limit.clamp(1, USAGE_SUMMARY_PAGE_MAX)
    }

    #[must_use]
    pub fn effective_from(&self) -> WallClockWithTz {
        let floor = self
            .to
            .as_datetime()
            .checked_sub_days(chrono::Days::new(USAGE_SUMMARY_RANGE_MAX_DAYS));
        match floor {
            Some(floor) => {
                let from = self.from.as_datetime();
                WallClockWithTz::from_datetime(if from < floor { floor } else { from })
            }
            None => self.from,
        }
    }

    #[must_use]
    pub fn effective_range_is_empty(&self) -> bool {
        self.effective_from().as_datetime() >= self.to.as_datetime()
    }
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageSummaryRepository: Send + Sync {
    async fn query_usage_summary(
        &self,
        query: UsageSummaryQuery,
    ) -> Result<Vec<UsageSummaryRow>, InferenceTechnicalError>;
}
