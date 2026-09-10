//! Wall-clock timestamp with its creation offset.
//!
//! [`WallClockWithTz`] records when something happened in human time: a UTC
//! instant plus the fixed offset observed where it was created. It deliberately
//! holds no timezone identity (`Asia/Tokyo`, DST rules, ...): schedule math
//! that needs those keeps the identifier in domain types. It is never a
//! revision substitute and never a monotonic clock: the system wall clock can
//! move backwards (NTP correction, manual changes), so currentness is decided
//! by comparing `(identity, revision)` or `(lifecycle, generation)` pairs
//! under their owners, never by comparing timestamps
//! (correspondence-identity §4.2). Elapsed-time measurement belongs to
//! [`std::time::Instant`], not here.

use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};

/// Wall-clock timestamp plus the fixed offset observed at creation.
///
/// The offset shapes rendering and schedule math, but it is not a timezone
/// identity and it does not participate in equality: [`PartialEq`], [`Eq`],
/// and [`Hash`] follow chrono's `DateTime<FixedOffset>` semantics, which
/// compare the represented instant. The same instant written with different
/// offsets compares equal. Timestamps serve explanation, display, and
/// schedule computation only.
///
/// ```
/// use ene_primitive::clock::WallClockWithTz;
///
/// let parsed = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00");
/// assert!(parsed.is_ok());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WallClockWithTz(DateTime<FixedOffset>);

impl WallClockWithTz {
    /// Captures the current wall-clock time with the local offset.
    ///
    /// The reading can move backwards between calls (NTP correction, manual
    /// clock changes): never use successive readings as a monotonic order.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// let taken = WallClockWithTz::now();
    /// let rendered = taken.to_rfc3339();
    /// assert!(WallClockWithTz::parse_rfc3339(&rendered).is_ok());
    /// ```
    #[must_use]
    pub fn now() -> Self {
        Self(Local::now().fixed_offset())
    }

    /// Wraps an existing timestamp, preserving its offset.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// let parsed = WallClockWithTz::parse_rfc3339("2026-01-02T03:04:05+09:00");
    /// assert!(parsed.is_ok());
    /// if let Ok(clock) = parsed {
    ///     assert_eq!(WallClockWithTz::from_datetime(clock.as_datetime()), clock);
    /// }
    /// ```
    #[must_use]
    pub fn from_datetime(value: DateTime<FixedOffset>) -> Self {
        Self(value)
    }

    /// Returns the wrapped timestamp, offset included.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// let parsed = WallClockWithTz::parse_rfc3339("2026-01-02T03:04:05Z");
    /// assert!(parsed.is_ok());
    /// if let Ok(clock) = parsed {
    ///     assert_eq!(WallClockWithTz::from_datetime(clock.as_datetime()), clock);
    /// }
    /// ```
    #[must_use]
    pub fn as_datetime(&self) -> DateTime<FixedOffset> {
        self.0
    }

    /// Renders the timestamp as RFC 3339, preserving the creation offset.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// let parsed = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00");
    /// assert!(parsed.is_ok());
    /// if let Ok(clock) = parsed {
    ///     assert_eq!(clock.to_rfc3339(), "2026-09-08T12:00:00+09:00");
    /// }
    /// ```
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0.to_rfc3339()
    }

    /// Parses RFC 3339 text, preserving the offset it carries.
    ///
    /// # Errors
    ///
    /// Returns [`chrono::ParseError`] when `value` is not valid RFC 3339.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// assert!(WallClockWithTz::parse_rfc3339("not a timestamp").is_err());
    /// ```
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
