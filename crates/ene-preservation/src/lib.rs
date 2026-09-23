mod operation;
mod request;
pub use operation::*;
pub use request::*;

mod participant;
pub use participant::*;

mod completion;
pub use completion::*;

use ene_primitive::RawId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionOperationId(RawId);

impl DeletionOperationId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeletionSweepGeneration(u64);

impl DeletionSweepGeneration {
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErasureConditionRef {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
}
