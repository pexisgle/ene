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

    pub fn parse_rfc3339(value: &str) -> Result<Self, chrono::ParseError> {
        DateTime::parse_from_rfc3339(value).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::WallClockWithTz;

    #[test]
    fn preserves_offset_through_parse_and_render() {
        let clock = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
            .expect("offset timestamp must parse");
        assert_eq!(clock.to_rfc3339(), "2026-09-08T12:00:00+09:00");
    }

    #[test]
    fn rejects_non_rfc3339_input() {
        assert!(WallClockWithTz::parse_rfc3339("not a timestamp").is_err());
    }

    #[test]
    fn same_instant_with_different_offsets_compares_equal() {
        let tokyo = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00")
            .expect("offset timestamp must parse");
        let utc = WallClockWithTz::parse_rfc3339("2026-09-08T03:00:00+00:00")
            .expect("UTC timestamp must parse");
        assert_eq!(
            tokyo, utc,
            "equality follows the instant, not the stored offset"
        );
    }

    #[test]
    fn wraps_and_returns_the_same_instant() {
        let clock = WallClockWithTz::parse_rfc3339("2026-01-02T03:04:05Z")
            .expect("UTC timestamp must parse");
        let rebuilt = WallClockWithTz::from_datetime(clock.as_datetime());
        assert_eq!(rebuilt, clock);
    }
}
