use crate::ids::{SessionId, SoulId, UsageId};
use serde::{Deserialize, Serialize};

/// One usage-ledger row before seq assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUsage {
    pub session_id: SessionId,
    pub soul_id: SoulId,
    pub lane: String,
    pub task: String,
    pub provider: String,
    pub model: String,
    pub entry_seq: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub cost_micro_usd: Option<i64>,
    pub adjustment: bool,
}

/// Persisted usage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRow {
    pub id: UsageId,
    pub session_id: SessionId,
    pub seq: u64,
    pub soul_id: SoulId,
    pub lane: String,
    pub task: String,
    pub provider: String,
    pub model: String,
    pub entry_seq: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    pub cost_micro_usd: Option<i64>,
    pub adjustment: bool,
    pub created_at: String,
}

/// Aggregated ledger totals.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub rows: u64,
}
