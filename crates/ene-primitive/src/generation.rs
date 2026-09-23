use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GenerationInner(u64);

impl GenerationInner {
    #[must_use]
    pub fn first() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}
