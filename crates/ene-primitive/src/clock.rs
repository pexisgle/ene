use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WallClockWithTz(DateTime<FixedOffset>);

impl WallClockWithTz {
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

    #[must_use]
    pub fn to_rfc3339_secs(&self) -> String {
        self.0.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    }

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
    fn second_precision_rendering_is_fixed_width() {
        let clock = WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00.987654321+09:00")
            .expect("offset timestamp must parse");
        assert_eq!(clock.to_rfc3339_secs(), "2026-09-08T12:00:00+09:00");
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
    fn utc_rendering_is_canonical_and_converts_the_offset() {
        let tokyo = WallClockWithTz::parse_rfc3339("2026-09-12T10:00:00.123456789+09:00")
            .expect("offset timestamp must parse");
        assert_eq!(
            tokyo.to_rfc3339_utc(),
            "2026-09-12T01:00:00.123456789Z",
            "the canonical rendering converts to UTC and keeps nanoseconds"
        );
        let utc = WallClockWithTz::parse_rfc3339("2026-09-12T01:00:00.123456789Z")
            .expect("UTC timestamp must parse");
        assert_eq!(tokyo.to_rfc3339_utc(), utc.to_rfc3339_utc());
    }

    #[test]
    fn utc_rendering_lexical_order_matches_instant_order() {
        let earlier = WallClockWithTz::parse_rfc3339("2026-09-12T10:00:00+09:00")
            .expect("offset timestamp must parse");
        let later = WallClockWithTz::parse_rfc3339("2026-09-12T00:30:00-05:00")
            .expect("offset timestamp must parse");
        assert!(
            earlier.as_datetime() < later.as_datetime(),
            "fixture premise: the instants order this way"
        );
        assert!(
            earlier.to_rfc3339_utc() < later.to_rfc3339_utc(),
            "canonical UTC text orders exactly like the instants"
        );
        assert!(
            earlier.to_rfc3339() > later.to_rfc3339(),
            "fixture premise: raw offset renderings would misorder lexically"
        );
    }
}
