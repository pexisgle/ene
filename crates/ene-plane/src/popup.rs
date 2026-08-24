use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::error::PlaneError;
use crate::request::AuthzRequest;

/// User (or test) answer to a popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupDecision {
    Allow,
    Deny,
    AllowAndRemember,
}

impl PopupDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::AllowAndRemember => "allow_and_remember",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, PlaneError> {
        match raw {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            "allow_and_remember" => Ok(Self::AllowAndRemember),
            other => Err(PlaneError::UnknownApproval(other.to_owned())),
        }
    }
}

/// Delivers an approval popup to clients (P-905).
#[async_trait]
pub trait PopupSink: Send + Sync {
    async fn ask(&self, req: &AuthzRequest) -> PopupDecision;

    async fn ask_timed(&self, req: &AuthzRequest, timeout: Duration) -> PopupDecision {
        match tokio::time::timeout(timeout, self.ask(req)).await {
            Ok(decision) => decision,
            Err(_) => PopupDecision::Deny,
        }
    }
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

/// One outstanding popup waiting for any connected client.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub id: String,
    pub tool: String,
    pub target: String,
    pub side_effects: Vec<String>,
    /// Model call this approval gates.
    pub call_id: String,
}

struct PendingSlot {
    view: PendingApproval,
    tx: Option<oneshot::Sender<PopupDecision>>,
}

/// Callback fired when a popup is inserted (core bus, tests).
pub type AskCallback = Arc<dyn Fn(&PendingApproval) + Send + Sync>;

/// First-writer popup sink: every client sees the ask; the first `respond` wins.
pub struct PendingPopup {
    inner: Mutex<HashMap<String, PendingSlot>>,
    resolved: Mutex<HashSet<String>>,
    on_ask: Mutex<Option<AskCallback>>,
}

impl PendingPopup {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            resolved: Mutex::new(HashSet::new()),
            on_ask: Mutex::new(None),
        }
    }

    /// Notify listeners (HTTP/WS bus) when a popup is inserted.
    pub fn set_on_ask(&self, callback: AskCallback) {
        *self.on_ask.lock() = Some(callback);
    }

    fn fire_on_ask(&self, view: &PendingApproval) {
        let callback = self.on_ask.lock().clone();
        if let Some(callback) = callback {
            callback(view);
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<PendingApproval> {
        self.inner
            .lock()
            .values()
            .map(|slot| slot.view.clone())
            .collect()
    }

    pub fn respond(&self, id: &str, decision: PopupDecision) -> Result<(), PlaneError> {
        if self.resolved.lock().contains(id) {
            return Err(PlaneError::AlreadyResolved(id.to_owned()));
        }
        let mut inner = self.inner.lock();
        let Some(mut slot) = inner.remove(id) else {
            return Err(PlaneError::UnknownApproval(id.to_owned()));
        };
        drop(inner);
        self.resolved.lock().insert(id.to_owned());
        if let Some(tx) = slot.tx.take()
            && tx.send(decision).is_err()
        {
            // Receiver already timed out.
        }
        Ok(())
    }

    pub fn cancel_timed_out(&self, id: &str) {
        self.inner.lock().remove(id);
    }
}

impl Default for PendingPopup {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PopupSink for PendingPopup {
    async fn ask(&self, req: &AuthzRequest) -> PopupDecision {
        let id = Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel();
        let view = PendingApproval {
            id: id.clone(),
            tool: req.tool.clone(),
            target: req.target.clone(),
            side_effects: req.side_effects.clone(),
            call_id: req.call_id.clone(),
        };
        self.inner.lock().insert(
            id.clone(),
            PendingSlot {
                view: view.clone(),
                tx: Some(tx),
            },
        );
        self.fire_on_ask(&view);
        let outcome = match rx.await {
            Ok(decision) => decision,
            Err(_) => PopupDecision::Deny,
        };
        self.inner.lock().remove(&id);
        self.resolved.lock().insert(id);
        outcome
    }

    async fn ask_timed(&self, req: &AuthzRequest, timeout: Duration) -> PopupDecision {
        let id = Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel();
        let view = PendingApproval {
            id: id.clone(),
            tool: req.tool.clone(),
            target: req.target.clone(),
            side_effects: req.side_effects.clone(),
            call_id: req.call_id.clone(),
        };
        self.inner.lock().insert(
            id.clone(),
            PendingSlot {
                view: view.clone(),
                tx: Some(tx),
            },
        );
        self.fire_on_ask(&view);
        let decision = if let Ok(Ok(decision)) = tokio::time::timeout(timeout, rx).await {
            decision
        } else {
            self.cancel_timed_out(&id);
            PopupDecision::Deny
        };
        self.resolved.lock().insert(id);
        decision
    }
}

/// Wait up to `timeout` for a popup. Timeout is deny.
#[expect(dead_code, reason = "public helper retained for external callers")]
pub async fn ask_with_timeout(
    sink: &dyn PopupSink,
    req: &AuthzRequest,
    timeout: Duration,
) -> PopupDecision {
    sink.ask_timed(req, timeout).await
}
