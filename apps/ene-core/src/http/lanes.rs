use std::collections::HashMap;
use std::sync::Arc;

use ene_kernel::{ConversationModel, KernelError, LaneHandle};
use ene_session::{SessionId, TurnId};
use parking_lot::Mutex;

use crate::{CoreDaemon, CoreError};

/// One dialogue lane per session. HTTP handlers share these handles.
pub struct LaneHub {
    lanes: Mutex<HashMap<SessionId, LaneHandle>>,
    turns: Mutex<HashMap<TurnId, SessionId>>,
    model: Arc<dyn ConversationModel>,
}

impl LaneHub {
    #[must_use]
    pub fn new(model: Arc<dyn ConversationModel>) -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
            turns: Mutex::new(HashMap::new()),
            model,
        }
    }

    pub fn get_or_open(
        &self,
        core: &CoreDaemon,
        session: SessionId,
    ) -> Result<LaneHandle, CoreError> {
        let meta = core.store().get_session(session)?;
        if meta.ended_at.is_some() {
            self.forget(session);
            return Err(KernelError::Closed.into());
        }
        if let Some(lane) = self.lanes.lock().get(&session) {
            return Ok(lane.clone());
        }
        let lane = core.open_lane(meta.soul_id, session, Arc::clone(&self.model));
        self.lanes.lock().insert(session, lane.clone());
        Ok(lane)
    }

    #[must_use]
    pub fn all(&self) -> Vec<LaneHandle> {
        self.lanes.lock().values().cloned().collect()
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.lanes.lock().len()
    }

    pub fn remember_turn(&self, turn: TurnId, session: SessionId) {
        self.turns.lock().insert(turn, session);
    }

    pub fn session_for_turn(&self, turn: TurnId) -> Option<SessionId> {
        self.turns.lock().get(&turn).copied()
    }

    /// Abort a running turn, stop the actor, and drop the cache entry.
    pub async fn close(&self, session: SessionId) {
        if let Some(handle) = self.take(session) {
            drop(handle.abort().await);
            drop(handle.shutdown().await);
        }
    }

    pub async fn reset(&self) {
        let handles: Vec<LaneHandle> = {
            self.turns.lock().clear();
            self.lanes
                .lock()
                .drain()
                .map(|(_, handle)| handle)
                .collect()
        };
        for handle in handles {
            drop(handle.abort().await);
            drop(handle.shutdown().await);
        }
    }

    pub fn any_busy(&self, core: &CoreDaemon) -> bool {
        for lane in self.all() {
            let Ok(events) = core.store().load_events(lane.session_id(), 0) else {
                continue;
            };
            if !ene_session::open_turns(&events).is_empty() {
                return true;
            }
        }
        false
    }

    fn take(&self, session: SessionId) -> Option<LaneHandle> {
        self.turns.lock().retain(|_, owner| *owner != session);
        self.lanes.lock().remove(&session)
    }

    fn forget(&self, session: SessionId) {
        if let Some(handle) = self.take(session) {
            drop(tokio::spawn(async move {
                drop(handle.abort().await);
                drop(handle.shutdown().await);
            }));
        }
    }
}
