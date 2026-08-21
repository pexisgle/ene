use crate::CompanionError;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Auxiliary-LLM task category (D-15). Callers pick the category; the
/// provider seam binds a model per category without the consumer branching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyTask {
    Affect,
    MemoryExtract,
    ProactiveDecision,
    ScreenSummary,
}

impl ClassifyTask {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Affect => "affect",
            Self::MemoryExtract => "memory_extract",
            Self::ProactiveDecision => "proactive_decision",
            Self::ScreenSummary => "screen_summary",
        }
    }
}

/// Cheap structured-output model used for classification, extraction, and
/// proactive decisions. Failures must be treated as silence / skip by callers.
#[async_trait]
pub trait ClassifyModel: Send + Sync {
    async fn complete_json(
        &self,
        task: ClassifyTask,
        input: &str,
    ) -> Result<String, CompanionError>;
}

/// Scripted classifier for tests and headless boots (fail-closed when empty).
#[derive(Debug, Default)]
pub struct ScriptedClassify {
    replies: Mutex<VecDeque<String>>,
    last_input: Mutex<Option<String>>,
}

impl ScriptedClassify {
    #[must_use]
    pub fn new(replies: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().map(Into::into).collect()),
            last_input: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn silent() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn last_input(&self) -> Option<String> {
        self.last_input.lock().clone()
    }
}

#[async_trait]
impl ClassifyModel for ScriptedClassify {
    async fn complete_json(
        &self,
        _task: ClassifyTask,
        input: &str,
    ) -> Result<String, CompanionError> {
        *self.last_input.lock() = Some(input.to_owned());
        self.replies
            .lock()
            .pop_front()
            .ok_or_else(|| CompanionError::Classify("no scripted reply".to_owned()))
    }
}
