//! Wall-clock timestamp with its creation timezone.
//!
//! [`WallClockWithTz`] records when something happened in human time. It is
//! never a revision substitute: currentness is decided by comparing
//! `(identity, revision)` or `(lifecycle, generation)` pairs under their
//! owners, never by comparing timestamps (correspondence-identity §4.2).

use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};

/// Wall-clock timestamp plus the timezone where it was created.
///
/// The offset is part of the value: rendering and schedule math keep the
/// creation timezone instead of silently normalising to UTC. Timestamps serve
/// explanation, display, and schedule computation only.
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
    /// Captures the current wall-clock time with the local timezone.
    ///
    /// ```
    /// use ene_primitive::clock::WallClockWithTz;
    ///
    /// let first = WallClockWithTz::now();
    /// let second = WallClockWithTz::now();
    /// assert!(first.as_datetime() <= second.as_datetime());
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
        let parsed = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00");
        assert!(parsed.is_ok());
        if let Ok(clock) = parsed {
            assert_eq!(clock.to_rfc3339(), "2026-09-08T12:00:00+09:00");
        }
    }

    #[test]
    fn rejects_non_rfc3339_input() {
        assert!(WallClockWithTz::parse_rfc3339("not a timestamp").is_err());
    }

    #[test]
    fn now_never_runs_backwards_between_two_reads() {
        let first = WallClockWithTz::now();
        let second = WallClockWithTz::now();
        assert!(first.as_datetime() <= second.as_datetime());
    }

    #[test]
    fn wraps_and_returns_the_same_instant() {
        let parsed = WallClockWithTz::parse_rfc3339("2026-01-02T03:04:05Z");
        assert!(parsed.is_ok());
        if let Ok(clock) = parsed {
            let rebuilt = WallClockWithTz::from_datetime(clock.as_datetime());
            assert_eq!(rebuilt, clock);
        }
    }
}
