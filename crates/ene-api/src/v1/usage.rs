use serde::{Deserialize, Serialize};

use super::refs::UsageCursorWire;

pub const USAGE_PAGE_LIMIT_MAX: u32 = 50;
pub const USAGE_PAGE_LIMIT_DEFAULT: u32 = 50;

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
    #[serde(default)]
    pub cursor: Option<UsageCursorWire>,
    #[serde(default)]
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
    pub mark: String,
    pub scope: String,
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
