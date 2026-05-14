use std::sync::Arc;
use ene_ai_core::{
    config::AiSettings,
    session::ConversationSession,
    PendingSplitTask,
    tool::ToolRegistry,
};

pub struct AppContext {
    pub settings: AiSettings,
    pub session: ConversationSession,
    pub registry: Arc<dyn ToolRegistry>,
    pub pending_split: Option<PendingSplitTask>,
}
