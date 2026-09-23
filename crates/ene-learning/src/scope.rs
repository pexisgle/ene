use ene_primitive::RawId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LearningScope {
    Companion { companion: RawId },
}

impl LearningScope {
    #[must_use]
    pub fn companion(companion: RawId) -> Self {
        Self::Companion { companion }
    }

    #[must_use]
    pub fn companion_id(&self) -> RawId {
        match self {
            Self::Companion { companion } => *companion,
        }
    }
}
