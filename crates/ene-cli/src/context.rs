use ene_ai_core::{
    PendingSplitTask, config::AiSettings, session::ConversationSession, tools::ToolRegistry,
};
use std::sync::Arc;

pub struct AppContext {
    pub settings: AiSettings,
    pub session: ConversationSession,
    pub registry: Arc<dyn ToolRegistry>,
    pub pending_split: Option<PendingSplitTask>,
}
