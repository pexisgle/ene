//! First-party usage / cost / cap read and control wire surface
//! (`usage-cost-cap` §16/§17).
//!
//! The read is bounded: `limit` is `1..=USAGE_PAGE_LIMIT_MAX` (default
//! [`USAGE_PAGE_LIMIT_DEFAULT`]) and the requested period is clamped Host-side
//! to the owner's maximum span, so a management surface can never ask for an
//! unbounded scan. The cursor is Host-issued, connection-bound, and bound to
//! the query that produced it; a foreign or reused cursor answers
//! [`UsageSummaryResponse::StaleBaseView`] rather than a fabricated page.
//!
//! No body text, prompt, output, or credential value travels this surface:
//! a row carries attribution (provider/model/consumer/purpose), settlement
//! status, token counts, cost components, and the cap amount its state counts.
//!
//! Cap mutation is not a payload of its own: it is a management intent whose
//! target the shared `cap:` grammar names
//! ([`super::management::usage_cap_target`]) and whose `base_view` is the
//! opaque mark of one [`UsageCapView`]. The Host re-checks the current
//! authenticated connection, the base view, and the cap revision before the
//! permission-owned command runs.

use serde::{Deserialize, Serialize};

use super::refs::UsageCursorWire;

/// Maximum rows one usage summary page may contain.
pub const USAGE_PAGE_LIMIT_MAX: u32 = 50;

/// Page size used when the request omits `limit`.
pub const USAGE_PAGE_LIMIT_DEFAULT: u32 = 50;

/// One bounded usage summary query.
///
/// `from` is inclusive and `to` exclusive over the attempt admission instant
/// (RFC 3339 text). The Host clamps the span and the row count at the owner
/// boundary; `from`/`to` are display filters, never authority. `consumer`,
/// `purpose`, and `status` are the owner vocabularies, parsed Host-side
/// closed-world (an unknown name is `UnsupportedFieldValue`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageSummaryRequest {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub consumer: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Host-issued cursor continuing a previous page of the same query.
    #[serde(default)]
    pub cursor: Option<UsageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One exact amount on the wire, in micro-currency units.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageMoneyView {
    pub currency: String,
    pub micros: u64,
}

/// Reported token counts. Cached input is a subset of input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageTokenUsageView {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

/// Cost components and total, derived from the immutable pricing snapshot the
/// ticket was admitted under.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageCostView {
    pub input: UsageMoneyView,
    pub cached_input: UsageMoneyView,
    pub output: UsageMoneyView,
    pub total: UsageMoneyView,
}

/// One ticket's usage summary row. No bodies or secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummaryRowView {
    pub provider: String,
    pub model: String,
    /// Owner consumer name (`companion_dialogue`, `companion_learning`,
    /// `task_agent`).
    pub consumer: String,
    /// Owner purpose name (`dialogue_response`, `memory_formation`,
    /// `task_agent_turn`).
    pub purpose: String,
    /// Settlement status: `reported`, `unknown`, or `reserved`.
    pub status: String,
    /// Present exactly for `reported`; `None` is never zero.
    pub tokens: Option<UsageTokenUsageView>,
    /// `None` means no amount is known for this row (not settled yet, or a
    /// reported usage no reviewed rate covered); it is never zero.
    pub cost: Option<UsageCostView>,
    /// The amount a non-reported state counts against its caps: the reserved
    /// upper bound. `None` for `reported` (cap accounting counts the actual
    /// committed cost instead) and when no reservation exists.
    pub reserved: Option<UsageMoneyView>,
    /// Admission instant (owner rendering).
    pub started_at: String,
}

/// One cap slot's opaque currentness mark and, when a cap row exists, its
/// limit and current-window consumption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCapView {
    /// Opaque mark for this exact `(scope, provider, window)` slot. Echo it
    /// as a cap intent's `base_view`; it names the revision the reader saw,
    /// or the none state when `stored` is `None`.
    pub mark: String,
    /// `system` or `provider`.
    pub scope: String,
    /// Provider the slot scopes, or `None` for the system scope.
    pub provider: Option<String>,
    /// `daily_utc` or `monthly_utc`.
    pub window: String,
    /// The stored cap, or `None` when this slot has no cap row.
    pub stored: Option<UsageCapStoredView>,
}

/// One stored cap's limit and current-window consumption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCapStoredView {
    pub limit: UsageMoneyView,
    pub consumption: UsageCapConsumptionView,
}

/// The durable reservation breakdown a new send is compared against. A
/// `consumed` here is exactly what cap admission reads; `held` says a new
/// positive reservation cannot fit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageCapConsumptionView {
    Known {
        reserved: UsageMoneyView,
        committed_reported: UsageMoneyView,
        committed_unknown: UsageMoneyView,
        consumed: UsageMoneyView,
        remaining: UsageMoneyView,
        held: bool,
    },
    /// The rows cannot answer a comparable amount; this is not zero
    /// consumption.
    Indeterminate,
}

/// One bounded usage summary page plus the cap slots the query's provider
/// filter names (the system scope is always included).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummaryPage {
    pub rows: Vec<UsageSummaryRowView>,
    /// Continues strictly past the last row while a further page may exist.
    pub next_cursor: Option<UsageCursorWire>,
    pub caps: Vec<UsageCapView>,
    /// Instant the cap windows were evaluated at (display fact only).
    pub evaluated_at: String,
}

/// Ok-side domain outcome of a usage summary read. `Unavailable` is the only
/// technical answer, and it never implies an empty page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageSummaryResponse {
    Page(UsageSummaryPage),
    /// The cursor was not issued on this connection for this query, or the
    /// filter premise moved: restart from the head.
    StaleBaseView {
        current: Option<UsageCursorWire>,
    },
    /// The usage read could not answer; nothing was read or changed.
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::{USAGE_PAGE_LIMIT_DEFAULT, USAGE_PAGE_LIMIT_MAX, UsageSummaryRequest};
    use crate::v1::refs::UsageCursorWire;

    #[test]
    fn page_bounds_match_the_wire_contract() {
        assert_eq!(USAGE_PAGE_LIMIT_MAX, 50);
        assert_eq!(USAGE_PAGE_LIMIT_DEFAULT, 50);
    }

    #[test]
    fn request_roundtrips_through_json_with_omitted_fields() {
        let json = r#"{"cursor":null}"#;
        let decoded: UsageSummaryRequest =
            serde_json::from_str(json).expect("optional fields may be omitted");
        assert_eq!(decoded.limit, None);
        assert_eq!(decoded.provider, None);
        assert_eq!(decoded.status, None);
        let decoded: UsageSummaryRequest = serde_json::from_str(
            r#"{"provider":"openai","status":"reserved","limit":10,"cursor":"cursor-1"}"#,
        )
        .expect("the request roundtrips");
        assert_eq!(decoded.provider.as_deref(), Some("openai"));
        assert_eq!(decoded.status.as_deref(), Some("reserved"));
        assert_eq!(decoded.limit, Some(10));
        assert_eq!(
            decoded.cursor,
            Some(UsageCursorWire(String::from("cursor-1")))
        );
    }
}
