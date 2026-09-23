//! Wall-clock timestamp with its creation offset.
//!
//! Deliberately holds no timezone identity (`Asia/Tokyo`, DST rules, ...):
//! schedule math that needs those keeps the identifier in domain types. It is
//! never a revision substitute and never a monotonic clock: the system wall
//! clock can move backwards (NTP correction, manual changes), so currentness
//! is decided by comparing `(identity, revision)` or `(lifecycle, generation)`
//! pairs under their owners, never by comparing timestamps
//! (correspondence-identity §4.2). Elapsed-time measurement belongs to
//! [`std::time::Instant`], not here.

use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};

/// Wall-clock timestamp plus the fixed offset observed at creation.
///
/// The offset shapes rendering and schedule math but is not a timezone
/// identity: equality and hashing follow the represented instant, so the same
/// instant written with different offsets compares equal. Timestamps serve
/// explanation, display, and schedule computation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WallClockWithTz(DateTime<FixedOffset>);

impl WallClockWithTz {
    /// Captures the current wall-clock time with the local offset.
    ///
    /// The reading can move backwards between calls (NTP correction, manual
    /// clock changes): never use successive readings as a monotonic order.
    #[must_use]
    pub fn now() -> Self {
        Self(Local::now().fixed_offset())
    }

    #[must_use]
    pub fn from_datetime(value: DateTime<FixedOffset>) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_datetime(&self) -> DateTime<FixedOffset> {
        self.0
    }

    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0.to_rfc3339()
    }

    /// Canonical UTC rendering with fixed nanosecond precision.
    ///
    /// Every instant renders as `YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`, so lexical
    /// order equals chronological order and storage/query layers can compare
    /// the rendered text without parsing. The display rendering
    /// ([`to_rfc3339`](Self::to_rfc3339)) keeps the creation offset; this one
    /// is for ordering and range filters only.
    #[must_use]
    pub fn to_rfc3339_utc(&self) -> String {
        use chrono::{SecondsFormat, Utc};
        self.0
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Nanos, true)
    }

    pub fn parse_rfc3339(value: &str) -> Result<Self, chrono::ParseError> {
        DateTime::parse_from_rfc3339(value).map(Self)
    }
}
