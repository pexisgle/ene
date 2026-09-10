//! Scope of one piece of Learning, as a semantic state distinct from
//! importance.

use ene_primitive::RawId;

/// Who may use one piece of Learning.
///
/// Stage 3 implements the Companion scope only. Global scope is intentionally
/// not representable yet: a Learning formed from one Companion's Experience
/// stays with that Companion, and importance or general usefulness can never
/// widen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LearningScope {
    Companion {
        /// Companion identity supplied as a premise; never converted into
        /// `ene-companion`'s newtype here.
        companion: RawId,
    },
}

impl LearningScope {
    #[must_use]
    pub fn companion(companion: RawId) -> Self {
        Self::Companion { companion }
    }

    /// The Companion this scope belongs to.
    #[must_use]
    pub fn companion_id(&self) -> RawId {
        match self {
            Self::Companion { companion } => *companion,
        }
    }
}
