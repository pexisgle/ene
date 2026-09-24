use serde::{Deserialize, Serialize};

use super::refs::{UsageCursorWire, ViewMarkWire};

pub const USAGE_PAGE_LIMIT_MAX: u32 = 50;
pub const USAGE_PAGE_LIMIT_DEFAULT: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageSummaryRequest {
    pub from: Option<String>,
    pub to: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub consumer: Option<String>,
    pub purpose: Option<String>,
    pub status: Option<String>,
    pub cursor: Option<UsageCursorWire>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageMoneyView {
    pub currency: String,
    pub micros: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageTokenUsageView {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsageCostView {
    pub input: UsageMoneyView,
    pub cached_input: UsageMoneyView,
    pub output: UsageMoneyView,
    pub total: UsageMoneyView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummaryRowView {
    pub provider: String,
    pub model: String,
    pub consumer: String,
    pub purpose: String,
    pub status: String,
    pub tokens: Option<UsageTokenUsageView>,
    pub cost: Option<UsageCostView>,
    pub reserved: Option<UsageMoneyView>,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCapView {
    pub mark: ViewMarkWire,
    pub provider: Option<String>,
    pub window: String,
    pub stored: Option<UsageCapStoredView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCapStoredView {
    pub limit: UsageMoneyView,
    pub consumption: UsageCapConsumptionView,
}

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
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummaryPage {
    pub rows: Vec<UsageSummaryRowView>,
    pub next_cursor: Option<UsageCursorWire>,
    pub caps: Vec<UsageCapView>,
    pub evaluated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageSummaryResponse {
    Page(UsageSummaryPage),
    StaleBaseView { current: Option<UsageCursorWire> },
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::{
        USAGE_PAGE_LIMIT_DEFAULT, USAGE_PAGE_LIMIT_MAX, UsageCapConsumptionView,
        UsageCapStoredView, UsageCapView, UsageCostView, UsageMoneyView, UsageSummaryPage,
        UsageSummaryRequest, UsageSummaryRowView, UsageTokenUsageView,
    };
    use crate::v1::refs::{UsageCursorWire, ViewMarkWire};

    fn money(micros: u64) -> UsageMoneyView {
        UsageMoneyView {
            currency: String::from("USD"),
            micros,
        }
    }

    #[test]
    fn usage_wire_contract_preserves_optional_fields_and_page_shape() {
        assert_eq!(USAGE_PAGE_LIMIT_MAX, 50);
        assert_eq!(USAGE_PAGE_LIMIT_DEFAULT, 50);

        let decoded: UsageSummaryRequest =
            serde_json::from_str(r#"{"cursor":null,"future_optional":{"revision":2}}"#)
                .expect("optional and unknown fields are accepted");
        assert_eq!(decoded.from, None);
        assert_eq!(decoded.to, None);
        assert_eq!(decoded.provider, None);
        assert_eq!(decoded.model, None);
        assert_eq!(decoded.consumer, None);
        assert_eq!(decoded.purpose, None);
        assert_eq!(decoded.status, None);
        assert_eq!(decoded.limit, None);
        assert_eq!(decoded.cursor, None);

        let decoded: UsageSummaryRequest = serde_json::from_str(
            r#"{"from":"2026-09-01","to":"2026-09-30","provider":"openai","model":"gpt-4o","consumer":"companion_dialogue","purpose":"dialogue_response","status":"reserved","limit":10,"cursor":"cursor-1"}"#,
        )
        .expect("the request roundtrips");
        assert_eq!(decoded.from.as_deref(), Some("2026-09-01"));
        assert_eq!(decoded.to.as_deref(), Some("2026-09-30"));
        assert_eq!(decoded.provider.as_deref(), Some("openai"));
        assert_eq!(decoded.model.as_deref(), Some("gpt-4o"));
        assert_eq!(decoded.consumer.as_deref(), Some("companion_dialogue"));
        assert_eq!(decoded.purpose.as_deref(), Some("dialogue_response"));
        assert_eq!(decoded.status.as_deref(), Some("reserved"));
        assert_eq!(decoded.limit, Some(10));
        assert_eq!(
            decoded.cursor,
            Some(UsageCursorWire(String::from("cursor-1")))
        );
        for malformed in [
            r#"{"from":7}"#,
            r#"{"provider":7}"#,
            r#"{"status":[]}"#,
            r#"{"limit":"10"}"#,
            r#"{"cursor":7}"#,
        ] {
            assert!(serde_json::from_str::<UsageSummaryRequest>(malformed).is_err());
        }

        let page = UsageSummaryPage {
            rows: vec![UsageSummaryRowView {
                provider: String::from("openai"),
                model: String::from("gpt-4o"),
                consumer: String::from("companion_dialogue"),
                purpose: String::from("dialogue_response"),
                status: String::from("reported"),
                tokens: Some(UsageTokenUsageView {
                    input_tokens: 10,
                    cached_input_tokens: 4,
                    output_tokens: 2,
                }),
                cost: Some(UsageCostView {
                    input: money(6),
                    cached_input: money(1),
                    output: money(4),
                    total: money(11),
                }),
                reserved: None,
                started_at: String::from("2026-09-17T00:00:00.000000000Z"),
            }],
            next_cursor: Some(UsageCursorWire(String::from("next-1"))),
            caps: vec![UsageCapView {
                mark: ViewMarkWire(String::from("usage-cap-system-daily_utc-rev-1")),
                provider: None,
                window: String::from("daily_utc"),
                stored: Some(UsageCapStoredView {
                    limit: money(1_000),
                    consumption: UsageCapConsumptionView::Known {
                        reserved: money(200),
                        committed_reported: money(100),
                        committed_unknown: money(200),
                        consumed: money(500),
                        remaining: money(500),
                        held: false,
                    },
                }),
            }],
            evaluated_at: String::from("2026-09-17T00:00:00.000000000Z"),
        };
        let json = serde_json::to_string(&page).expect("the page serializes");
        assert!(json.contains("companion_dialogue"));
        assert!(json.contains("cached_input"));
        assert!(json.contains("usage-cap-system-daily_utc-rev-1"));
        let back: UsageSummaryPage = serde_json::from_str(&json).expect("the page roundtrips");
        assert_eq!(back, page);
    }
}
