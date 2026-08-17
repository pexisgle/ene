use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crate::request::AuthzRequest;

/// User (or test) answer to a popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupDecision {
    Allow,
    Deny,
    AllowAndRemember,
}

/// Delivers an approval popup to clients (P-905).
#[async_trait]
pub trait PopupSink: Send + Sync {
    async fn ask(&self, req: &AuthzRequest) -> PopupDecision;
}

/// Predetermined answers for tests. Exhaustion means timeout/deny.
#[derive(Debug, Default)]
pub struct ScriptedPopup {
    answers: Mutex<VecDeque<PopupDecision>>,
}

impl ScriptedPopup {
    #[must_use]
    pub fn new(answers: impl IntoIterator<Item = PopupDecision>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
        }
    }

    #[must_use]
    pub fn deny_all() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl PopupSink for ScriptedPopup {
    async fn ask(&self, _req: &AuthzRequest) -> PopupDecision {
        self.answers
            .lock()
            .pop_front()
            .unwrap_or(PopupDecision::Deny)
    }
}

/// Wait up to `timeout` for a popup. Timeout is deny.
pub async fn ask_with_timeout(
    sink: &dyn PopupSink,
    req: &AuthzRequest,
    timeout: Duration,
) -> PopupDecision {
    match tokio::time::timeout(timeout, sink.ask(req)).await {
        Ok(decision) => decision,
        Err(_) => PopupDecision::Deny,
    }
}
