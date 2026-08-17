use std::collections::HashMap;
use std::sync::Arc;

use ene_kernel::{ConversationModel, LaneHandle};
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
        if let Some(lane) = self.lanes.lock().get(&session) {
            return Ok(lane.clone());
        }
        let meta = core.store().get_session(session)?;
        let lane = core.open_lane(meta.soul_id, session, Arc::clone(&self.model));
        self.lanes.lock().insert(session, lane.clone());
        Ok(lane)
    }

    #[must_use]
    pub fn all(&self) -> Vec<LaneHandle> {
        self.lanes.lock().values().cloned().collect()
    }

    pub fn remember_turn(&self, turn: TurnId, session: SessionId) {
        self.turns.lock().insert(turn, session);
    }

    pub fn session_for_turn(&self, turn: TurnId) -> Option<SessionId> {
        self.turns.lock().get(&turn).copied()
    }
}
