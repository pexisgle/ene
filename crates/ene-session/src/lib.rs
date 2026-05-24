pub mod config;
pub mod conversation_manager;
pub mod error;
pub mod session;
pub mod special_token;
pub mod utils;

pub use conversation_manager::{
    PendingSplitTask, SessionBoundary, SplitReason, SplitResult, SplitTaskInput, check_boundary,
    execute_split, generate_session_id, poll_split_result, spawn_split_task,
};
pub use error::SessionError;
pub use session::ConversationSession;
pub use special_token::{extract_emotion_from_token, split_text_and_special_tokens};
pub use utils::truncate;
pub use ene_config::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
pub use config::SessionConfig;
