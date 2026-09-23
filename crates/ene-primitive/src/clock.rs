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
