//! Bounded first-party usage summary query contract (`usage-cost-cap` §16).
//!
//! Stage 7 UI reads token/cost attribution through this contract instead of
//! the private database schema. The owner boundary is inference: it decides
//! what one usage row means (Reported / Unknown / Reserved), which pricing
//! snapshot prices it, and how much of the row the caller may read in one
//! page. The storage implementation applies the clamp to the SQL itself, so
//! `limit` bounds the rows read, not only the rows returned.
//!
//! A query is SELECT-only. Reading usage never settles a reservation, never
//! refreshes pricing, never mutates a cap, and never reconciles usage facts;
//! a non-terminal reservation appears as [`UsageSummaryStatus::Reserved`]
//! with its still-counted upper bound and is left exactly as it is.

use ene_permission::{ConsumerKind, PurposeKind};
use ene_primitive::WallClockWithTz;

use crate::InferenceTechnicalError;
use crate::InferenceTicketId;
use crate::cost::{Money, UsageCostFact};

/// Maximum rows one bounded usage summary page may contain. The clamp lives
/// here so the Host wire clamps and the storage query agree on one number.
pub const USAGE_SUMMARY_PAGE_MAX: u32 = 50;

/// Maximum span between the query's `from` and `to`, in days. A caller that
/// asks for a wider range is answered with the most recent
/// `USAGE_SUMMARY_RANGE_MAX_DAYS` of it rather than an unbounded scan; each
/// page is independently keyset-bounded as well.
pub const USAGE_SUMMARY_RANGE_MAX_DAYS: u64 = 366;

/// Default span used by the Host when a request names no lower bound.
pub const USAGE_SUMMARY_RANGE_DEFAULT_DAYS: u64 = 30;

/// Settlement state of one ticket's usage row.
///
/// The three are mutually exclusive by construction: a ticket is `Reported`
/// when its durable usage fact carries counts, `Unknown` when the external
/// consumption cannot be denied and no counts are known, and `Reserved` while
/// the provider I/O may still be running without a settled fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageSummaryStatus {
    /// The provider reported complete token counts.
    Reported,
    /// The external consumption cannot be denied; no counts are known.
    Unknown,
    /// No usage fact exists yet: the reservation is still non-terminal (or
    /// no cap applied to the route). Nothing is released or estimated.
    Reserved,
}

impl UsageSummaryStatus {
    /// Stable name shared by the wire projection and storage filters.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reported => "reported",
            Self::Unknown => "unknown",
            Self::Reserved => "reserved",
        }
    }

    /// Parses [`Self::as_str`], closed world.
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

/// Reported token counts of one settled ticket. Cached input is a subset of
/// input, never an additional count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportedTokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

/// One ticket's bounded usage summary row: attribution, settlement state,
/// tokens, cost, and the cap amount its state counts.
///
/// No prompt, output, or credential value is part of this row; the row names
/// the provider route, the consumer/purpose attribution, and the accounting
/// numbers only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummaryRow {
    /// Durable ticket identity the row summarizes.
    pub ticket: InferenceTicketId,
    pub provider: String,
    pub model: String,
    /// Consumer attribution resolved from the claimed attempt.
    pub consumer: ConsumerKind,
    /// Purpose binding of the claimed attempt.
    pub purpose: PurposeKind,
    pub status: UsageSummaryStatus,
    /// Reported counts, present exactly for [`UsageSummaryStatus::Reported`].
    pub tokens: Option<ReportedTokenUsage>,
    /// Projected cost fact, present once a usage fact settled. `None` means
    /// no settlement exists yet (the reserved row); it is never a zero cost.
    /// A `Reported` token row still answers
    /// [`UsageCostFact::Unknown`] when no reviewed rate priced it.
    pub cost: Option<UsageCostFact>,
    /// The amount this row's state counts against the applicable caps: the
    /// reserved upper bound for `Reserved` and `Unknown`, `None` for
    /// `Reported` (cap accounting counts its actual committed cost instead).
    pub reserved: Option<Money>,
    /// Admission instant the row is ordered and filtered by.
    pub started_at: WallClockWithTz,
}

/// Keyset premise of one page: the next page starts strictly older than
/// `(started_at, ticket)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsageSummaryCursor {
    pub started_at: WallClockWithTz,
    pub ticket: InferenceTicketId,
}

/// One bounded first-party usage summary query.
///
/// `from` is inclusive and `to` exclusive over the attempt admission instant.
/// The query's dimension filters (`provider`, `model`, `consumer`, `purpose`,
/// `status`) each narrow the rows; the storage implementation applies them in
/// SQL together with the keyset premise and the clamped `LIMIT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSummaryQuery {
    /// Inclusive lower bound.
    pub from: WallClockWithTz,
    /// Exclusive upper bound.
    pub to: WallClockWithTz,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub consumer: Option<ConsumerKind>,
    pub purpose: Option<PurposeKind>,
    pub status: Option<UsageSummaryStatus>,
    /// Keyset position: only rows strictly older than this are returned.
    pub after: Option<UsageSummaryCursor>,
    pub limit: u32,
}

impl UsageSummaryQuery {
    /// The row bound actually applied: `1..=USAGE_SUMMARY_PAGE_MAX`. A zero
    /// or oversized request is clamped at the owner boundary, so the storage
    /// query can never read an unbounded page.
    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        self.limit.clamp(1, USAGE_SUMMARY_PAGE_MAX)
    }

    /// The inclusive lower bound actually applied: never more than
    /// [`USAGE_SUMMARY_RANGE_MAX_DAYS`] before `to`, so a query can never
    /// scan an unbounded history in one page.
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
            // An unrepresentable floor is the calendar extreme: keep the
            // caller's bound rather than inventing one.
            None => self.from,
        }
    }

    /// Whether the effective range contains any instant. An empty or
    /// inverted range is an empty page, never an unbounded read.
    #[must_use]
    pub fn effective_range_is_empty(&self) -> bool {
        self.effective_from().as_datetime() >= self.to.as_datetime()
    }
}

/// Owner boundary of the bounded first-party usage summary read.
///
/// The implementation clamps `limit` and the range at the storage boundary
/// ([`UsageSummaryQuery::effective_limit`],
/// [`UsageSummaryQuery::effective_from`]) and applies both to the SQL, so the
/// bound is on the rows read. Every method is SELECT-only: no reservation
/// settlement, pricing refresh, cap mutation, or usage reconciliation may be
/// a side effect of a read.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageSummaryRepository: Send + Sync {
    /// Reads one bounded page of usage summary rows, newest first by
    /// `(started_at, ticket)`.
    ///
    /// Filters: the half-open admission range, provider, model, consumer,
    /// purpose, and settlement status. `after` continues strictly past a
    /// previously returned row. The read is SELECT-only.
    async fn query_usage_summary(
        &self,
        query: UsageSummaryQuery,
    ) -> Result<Vec<UsageSummaryRow>, InferenceTechnicalError>;
}

#[cfg(test)]
mod tests {
    use ene_primitive::WallClockWithTz;

    use super::{
        USAGE_SUMMARY_PAGE_MAX, USAGE_SUMMARY_RANGE_MAX_DAYS, UsageSummaryQuery, UsageSummaryStatus,
    };

    fn at(value: &str) -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339(value).expect("the fixture instant parses")
    }

    fn query(from: &str, to: &str, limit: u32) -> UsageSummaryQuery {
        UsageSummaryQuery {
            from: at(from),
            to: at(to),
            provider: None,
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            after: None,
            limit,
        }
    }

    #[test]
    fn status_vocabulary_is_closed_world() {
        assert_eq!(UsageSummaryStatus::Reported.as_str(), "reported");
        assert_eq!(UsageSummaryStatus::Unknown.as_str(), "unknown");
        assert_eq!(UsageSummaryStatus::Reserved.as_str(), "reserved");
        assert_eq!(
            UsageSummaryStatus::from_name("reported"),
            Some(UsageSummaryStatus::Reported)
        );
        assert_eq!(
            UsageSummaryStatus::from_name("reserved"),
            Some(UsageSummaryStatus::Reserved)
        );
        assert_eq!(UsageSummaryStatus::from_name("settled"), None);
        assert_eq!(UsageSummaryStatus::from_name(""), None);
    }

    #[test]
    fn limit_is_clamped_to_a_positive_bounded_page() {
        assert_eq!(
            query("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z", 0).effective_limit(),
            1
        );
        assert_eq!(
            query("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z", 7).effective_limit(),
            7
        );
        assert_eq!(
            query("2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z", u32::MAX).effective_limit(),
            USAGE_SUMMARY_PAGE_MAX
        );
    }

    #[test]
    fn range_is_clamped_to_the_maximum_span() {
        let clamped = query("2000-01-01T00:00:00Z", "2026-09-01T00:00:00Z", 10);
        let expected_floor = at("2026-09-01T00:00:00Z").as_datetime()
            - chrono::Days::new(USAGE_SUMMARY_RANGE_MAX_DAYS);
        assert_eq!(
            clamped.effective_from().as_datetime(),
            expected_floor,
            "the read keeps only the most recent bounded span"
        );
        // A range already inside the bound keeps its own lower bound.
        let inside = query("2026-08-31T00:00:00Z", "2026-09-01T00:00:00Z", 10);
        assert_eq!(inside.effective_from(), at("2026-08-31T00:00:00Z"));
        assert!(!inside.effective_range_is_empty());
    }

    #[test]
    fn inverted_or_empty_ranges_are_empty_pages_not_unbounded_reads() {
        let inverted = query("2026-09-02T00:00:00Z", "2026-09-01T00:00:00Z", 10);
        assert!(inverted.effective_range_is_empty());
        let empty = query("2026-09-01T00:00:00Z", "2026-09-01T00:00:00Z", 10);
        assert!(empty.effective_range_is_empty());
    }
}
